// The Remote page on a phone, end to end: a paired phone at 390 by 844 with a touch screen, played
// without leaving this machine through the debug build's phone seams (see phones.spec.ts). Every
// request the phone context makes carries the test peer header, so the server takes it for
// 192.168.1.50, and its pairing cookie; a finger is Chromium's own touch input, so `touch-action`
// and the pointer events a real phone makes are what the page sees.

import { expect, test, type Browser, type BrowserContext, type CDPSession, type Page } from "@playwright/test";

import { startServer, TEST_PEER_HEADER, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

const PHONE = { [TEST_PEER_HEADER]: "192.168.1.50:51000" };
const QUADRO = "loopback-0";
const STUDIO = "loopback-1";
/** Where the screenshots for the report go, when asked for (REMOTE_SCREENSHOTS=<directory>). */
const SHOTS = process.env["REMOTE_SCREENSHOTS"];

// Dry run, with no reports: what the page sets is all there is, so it stays set to be read back.
test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true, phoneSeams: true });
});

test.afterAll(async () => {
  await server?.stop();
});

const api = (path: string, init?: RequestInit) => fetch(`${server.url}/api/v1/${path}`, init);

async function revokeAll(): Promise<void> {
  const status = (await (await api("remote")).json()) as { phones: { id: string }[] };
  for (const phone of status.phones) await api(`remote/phones/${phone.id}`, { method: "DELETE" });
  await api("remote/pairing", { method: "DELETE" });
}

async function allowPhones(on: boolean): Promise<void> {
  expect((await api("remote", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ allow_phones: on }) })).status).toBe(200);
}

/** Pairs through the API, as the pair page would, and returns the token and the phone's id. */
async function pair(name: string): Promise<{ token: string; id: string }> {
  const started = (await (await api("remote/pairing", { method: "POST" })).json()) as { code: string };
  const answer = await api("remote/pair", { method: "POST", headers: { ...PHONE, "content-type": "application/json" }, body: JSON.stringify({ code: started.code, name }) });
  expect(answer.status).toBe(200);
  const body = (await answer.json()) as { token: string; phone: { id: string } };
  return { token: body.token, id: body.phone.id };
}

/** A phone: its size, a touch screen, the peer header on every request, and no pairing yet. */
function phoneContext(browser: Browser, colorScheme: "dark" | "light" = "dark"): Promise<BrowserContext> {
  return browser.newContext({ extraHTTPHeaders: PHONE, viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2, colorScheme });
}

/** A paired phone's context: the cookie the pair page would have left. */
async function pairedPhone(browser: Browser, colorScheme: "dark" | "light" = "dark"): Promise<{ context: BrowserContext; id: string }> {
  const { token, id } = await pair("Remote test");
  const context = await phoneContext(browser, colorScheme);
  await context.addCookies([{ name: "gazelle_token", value: token, url: server.url, httpOnly: true, sameSite: "Strict" }]);
  return { context, id };
}

/** Every request to a route that stays on the computer, and every refusal, as the page makes them. */
function watchRequests(page: Page): { localOnly: string[]; refused: string[] } {
  const seen = { localOnly: [] as string[], refused: [] as string[] };
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (/^\/api\/v1\/(update|window|aggregate|remote)(\/|$)/.test(path) && path !== "/api/v1/remote/pair") seen.localOnly.push(`${request.method()} ${path}`);
  });
  page.on("response", (response) => {
    if (response.status() === 401 || response.status() === 403) seen.refused.push(`${response.status()} ${response.url()}`);
  });
  return seen;
}

/** The commands the page sends over its socket, parsed. */
function watchCommands(page: Page): { command: string; device_id: string; args: Record<string, number> }[] {
  const sent: { command: string; device_id: string; args: Record<string, number> }[] = [];
  page.on("websocket", (socket) => {
    socket.on("framesent", (frame) => {
      if (typeof frame.payload !== "string") return;
      try {
        const parsed = JSON.parse(frame.payload) as { command?: string; device_id?: string; args?: Record<string, number> };
        if (typeof parsed.command === "string") sent.push({ command: parsed.command, device_id: parsed.device_id ?? "", args: parsed.args ?? {} });
      } catch {
        // Not a command.
      }
    });
  });
  return sent;
}

/** A finger, through Chromium's own touch input: what a phone's touch screen delivers. */
class Finger {
  readonly #cdp: CDPSession;
  constructor(cdp: CDPSession) {
    this.#cdp = cdp;
  }
  static async on(page: Page): Promise<Finger> {
    return new Finger(await page.context().newCDPSession(page));
  }
  async down(x: number, y: number): Promise<void> {
    await this.#cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y }] });
  }
  async move(x: number, y: number): Promise<void> {
    await this.#cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x, y }] });
  }
  async up(): Promise<void> {
    await this.#cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  }
  /** Down at `from`, along to `to` in `steps` moves `ms` apart, and up. */
  async drag(from: { x: number; y: number }, to: { x: number; y: number }, steps: number, ms: number): Promise<void> {
    await this.down(from.x, from.y);
    for (let i = 1; i <= steps; i++) {
      await this.move(from.x + ((to.x - from.x) * i) / steps, from.y + ((to.y - from.y) * i) / steps);
      if (ms > 0) await new Promise((resolve) => setTimeout(resolve, ms));
    }
    await this.up();
  }
}

const channel = (id: string, name: string, slot: number, source: number, main: number, sends: number[] = []) => ({ id, name, slot, sends, source: { group: 0, channel: source }, main_mix: main });

/** A Quadro with channels in two mixes, and a Studio+ with one. */
async function layout(): Promise<void> {
  await putWorkspace(server, {
    aliases: { [QUADRO]: "Quadro", [STUDIO]: "Studio+" },
    mixers: {
      [QUADRO]: {
        mixes: [{ name: "Monitors" }, { name: "Cue" }],
        groups: [],
        channels: [channel("a", "Kick", 0, 0, 0), channel("b", "Snare", 1, 1, 0), channel("c", "Vox", 2, 2, 0, [1]), channel("d", "Click", 3, 3, 1)],
      },
      [STUDIO]: { mixes: [{ name: "Main" }], groups: [], channels: [channel("k", "Guitar", 0, 0, 0)] },
    },
  });
}

test.beforeEach(async () => {
  await revokeAll();
  await allowPhones(true);
  await layout();
});

/** The Remote page itself: the sidebar's drawer holds a Control Room with the same test ids. */
const inRemote = (page: Page) => page.locator("ga-remote");

const box = async (page: Page, testId: string) => {
  // Brought into view first: the recording transport at the top can push a control below the fold.
  await inRemote(page).getByTestId(testId).scrollIntoViewIfNeeded();
  const found = await inRemote(page).getByTestId(testId).boundingBox();
  if (found === null) throw new Error(`${testId} is not on screen`);
  return found;
};

test("a paired phone gets every section, and never asks for what stays on the computer", async ({ browser }) => {
  const { context } = await pairedPhone(browser);
  const page = await context.newPage();
  const requests = watchRequests(page);
  await page.goto(`${server.url}/#/remote/${QUADRO}`);
  await expect(page.getByTestId("connection")).toHaveAttribute("data-state", "open");

  // The device, and a switch between the two.
  await expect(inRemote(page).getByTestId(`remote-device-${QUADRO}`)).toHaveAttribute("aria-current", "page");
  await expect(inRemote(page).getByTestId(`remote-device-${STUDIO}`)).toBeVisible();
  // Monitoring: the Control Room's own outputs, and the Quadro's hard mute apart from them.
  for (const id of [0, 1, 2]) await expect(inRemote(page).getByTestId(`cr-volume-${id}`)).toBeVisible();
  await expect(inRemote(page).getByTestId("remote-hard-mute")).toHaveText("Hard mute");
  // Mix: a fader per channel in the mix, and the master last.
  await expect(inRemote(page).getByTestId("remote-mix").locator("ga-remote-strip")).toHaveCount(4);
  await expect(inRemote(page).getByTestId("remote-fader-master")).toBeVisible();
  // Inputs: each preamp with its type shown.
  await expect(inRemote(page).getByTestId("remote-preamps").locator(".preamp")).toHaveCount(4);
  await expect(inRemote(page).getByTestId("remote-type-0")).toHaveText(/Mic|Line|Hi-Z/);
  // The dock would repeat the Mix section, so it stays out of the way here.
  await expect(page.locator("ga-mixer-dock")).toBeHidden();
  // Remote is the first tab on a phone.
  const tabs = await page.locator("ga-header nav a").evaluateAll((links) => links.map((a) => ({ page: a.getAttribute("data-page"), x: a.getBoundingClientRect().x })).sort((a, b) => a.x - b.x));
  expect(tabs[0]?.page).toBe("remote");

  // Every thumb target is at least 44 px.
  const small = await page.evaluate(() => {
    const found: string[] = [];
    const visit = (root: ShadowRoot) => {
      for (const element of root.querySelectorAll("*")) {
        if (element.shadowRoot) visit(element.shadowRoot);
        if (!element.matches('button, select, [role="slider"], a[href]')) continue;
        const rect = element.getBoundingClientRect();
        if (rect.width > 0 && rect.height < 44) found.push(`${element.localName} ${element.getAttribute("data-testid") ?? element.getAttribute("aria-label") ?? ""} ${Math.round(rect.height)} px`);
      }
    };
    const remote = document.querySelector("ga-app")?.shadowRoot?.querySelector("ga-remote")?.shadowRoot;
    if (remote) visit(remote);
    return found;
  });
  expect(small).toEqual([]);

  // Switching device goes to the other one's remote, and still nothing local-only is asked.
  await inRemote(page).getByTestId(`remote-device-${STUDIO}`).tap();
  await expect(inRemote(page).getByTestId("cr-talkback")).toBeVisible();
  await expect(inRemote(page).getByTestId("remote-hard-mute")).toHaveCount(0);
  await page.waitForTimeout(500);
  expect(requests.localOnly).toEqual([]);
  expect(requests.refused).toEqual([]);
  await context.close();
});

test("a fader moves by a drag of the finger, not by a tap, and a flick cannot make it loud", async ({ browser }) => {
  const { context } = await pairedPhone(browser);
  const page = await context.newPage();
  const sent = watchCommands(page);
  await page.goto(`${server.url}/#/remote/${QUADRO}`);
  const fader = inRemote(page).getByTestId("remote-fader-1");
  await fader.scrollIntoViewIfNeeded();
  await expect(fader).toHaveAttribute("aria-valuenow", /-?\d+/);
  const finger = await Finger.on(page);

  // A known start: -40 dB, set with the keyboard (Home is the floor; forty steps up).
  await fader.focus();
  await fader.press("Home");
  for (let i = 0; i < 50; i++) await fader.press("ArrowUp");
  await expect(fader).toHaveAttribute("aria-valuenow", "-40");
  const start = sent.length;

  // A tap anywhere on it, even at the loud end, changes nothing.
  let at = await box(page, "remote-fader-1");
  await finger.down(at.x + at.width - 4, at.y + at.height / 2);
  await finger.up();
  await page.waitForTimeout(300);
  await expect(fader).toHaveAttribute("aria-valuenow", "-40");
  expect(sent.slice(start).filter((c) => c.command.includes("mixer"))).toEqual([]);

  // The reset asks first, and goes to -20 dB, never unity.
  const reset = inRemote(page).getByTestId("remote-reset-1");
  await reset.tap();
  await expect(reset).toHaveText("Confirm");
  await expect(fader).toHaveAttribute("aria-valuenow", "-40");
  await reset.tap();
  await expect(fader).toHaveAttribute("aria-valuenow", "-20");
  await expect(reset).toHaveText("-20");
  at = await box(page, "remote-fader-1");

  // A drag to the left makes it quieter, and sends each level.
  const from = sent.length;
  await finger.drag({ x: at.x + at.width / 2, y: at.y + at.height / 2 }, { x: at.x + at.width / 4, y: at.y + at.height / 2 }, 10, 16);
  await expect.poll(async () => Number(await fader.getAttribute("aria-valuenow"))).toBeLessThan(-20);
  const quieter = Number(await fader.getAttribute("aria-valuenow"));
  const levels = sent.slice(from).filter((c) => c.device_id === QUADRO && "level" in c.args).map((c) => c.args["level"]);
  expect(levels.length).toBeGreaterThan(0);
  expect(levels.at(-1)).toBe(-quieter);

  // A flick the whole width to the right adds only a few dB.
  await finger.drag({ x: at.x + 10, y: at.y + at.height / 2 }, { x: at.x + at.width - 4, y: at.y + at.height / 2 }, 4, 0);
  await page.waitForTimeout(200);
  const after = Number(await fader.getAttribute("aria-valuenow"));
  expect(after).toBeGreaterThanOrEqual(quieter);
  expect(after - quieter).toBeLessThanOrEqual(4);

  // A vertical swipe across it scrolls the page and leaves the level alone.
  const scrollTop = () => page.evaluate(() => document.querySelector("ga-app")?.shadowRoot?.querySelector("main")?.scrollTop ?? 0);
  const scrolled = await scrollTop();
  const now = await box(page, "remote-fader-1");
  await finger.drag({ x: now.x + now.width / 2, y: now.y + now.height / 2 }, { x: now.x + now.width / 2 + 2, y: now.y + now.height / 2 - 200 }, 10, 16);
  await page.waitForTimeout(300);
  await expect(fader).toHaveAttribute("aria-valuenow", String(after));
  expect(await scrollTop()).toBeGreaterThan(scrolled);
  await context.close();
});

test("the Monitor volume ignores a tap too, and 48V takes two taps", async ({ browser }) => {
  const { context } = await pairedPhone(browser);
  const page = await context.newPage();
  const sent = watchCommands(page);
  await page.goto(`${server.url}/#/remote/${QUADRO}`);
  const monitor = inRemote(page).getByTestId("cr-volume-0");
  await monitor.focus();
  await monitor.press("Home");
  await expect(monitor).toHaveAttribute("aria-valuetext", "-inf");
  const finger = await Finger.on(page);
  const at = await box(page, "cr-volume-0");
  await finger.down(at.x + at.width - 3, at.y + at.height / 2);
  await finger.up();
  await page.waitForTimeout(300);
  await expect(monitor).toHaveAttribute("aria-valuetext", "-inf");

  // Preamp 1 on Mic, 48V off, as the Inputs page would leave it.
  const card = inRemote(page).getByTestId("preamp-0");
  await card.scrollIntoViewIfNeeded();
  await expect(inRemote(page).getByTestId("remote-type-0")).toHaveText("Mic");
  const phantom = inRemote(page).getByTestId("pre-48v-0");
  await expect(phantom).toHaveAttribute("aria-pressed", "false");
  const before = sent.length;
  await phantom.tap();
  await expect(phantom).toHaveText("Confirm");
  await expect(phantom).toHaveAttribute("aria-pressed", "false");
  expect(sent.slice(before).filter((c) => c.command.includes("phantom") || "phantom" in c.args)).toEqual([]);
  await phantom.tap();
  await expect(phantom).toHaveAttribute("aria-pressed", "true");
  expect(sent.slice(before).length).toBeGreaterThan(0);
  // Off is one tap.
  await phantom.tap();
  await expect(phantom).toHaveAttribute("aria-pressed", "false");
  await context.close();
});

test("the mix follows the dock's mix choice, and the Remote's choice is the Mixer page's", async ({ browser }) => {
  const { context } = await pairedPhone(browser);
  const page = await context.newPage();
  // The Mixer page's Mix buttons choose Cue.
  await page.goto(`${server.url}/#/mixer/${QUADRO}`);
  await page.getByTestId("mix-1").tap();
  await page.goto(`${server.url}/#/remote/${QUADRO}`);
  await expect(inRemote(page).getByTestId("remote-mix-select")).toHaveValue("1");
  // Cue holds Vox (a send) and Click (its own), and its master.
  const names = inRemote(page).getByTestId("remote-mix").locator("ga-remote-strip");
  await expect(names).toHaveCount(3);
  await expect(inRemote(page).getByTestId("remote-strip-2")).toContainText("Vox");
  await expect(inRemote(page).getByTestId("remote-strip-3")).toContainText("Click");
  await expect(inRemote(page).getByTestId("remote-strip-master")).toContainText("Cue");

  // Back to Monitors from here, and the Mixer page follows.
  await inRemote(page).getByTestId("remote-mix-select").selectOption("0");
  await expect(names).toHaveCount(4);
  await page.goto(`${server.url}/#/mixer/${QUADRO}`);
  await expect(page.getByTestId("mix-0")).toHaveAttribute("aria-checked", "true");
  await context.close();
});

test("talkback on the Studio+ is held, not tapped", async ({ browser }) => {
  const { context } = await pairedPhone(browser);
  const page = await context.newPage();
  const sent = watchCommands(page);
  await page.goto(`${server.url}/#/remote/${STUDIO}`);
  const talk = inRemote(page).getByTestId("cr-talk");
  await expect(talk).toHaveAttribute("aria-pressed", "false");
  const finger = await Finger.on(page);
  const at = await box(page, "cr-talk");
  await finger.down(at.x + at.width / 2, at.y + at.height / 2);
  await expect(talk).toHaveAttribute("aria-pressed", "true");
  // Held for a while, with the finger drifting a little, it keeps talking.
  await finger.move(at.x + at.width / 2 + 3, at.y + at.height / 2 + 3);
  await page.waitForTimeout(700);
  await expect(talk).toHaveAttribute("aria-pressed", "true");
  await finger.up();
  await expect(talk).toHaveAttribute("aria-pressed", "false");
  const talks = sent.filter((c) => c.command === "set_talk").map((c) => c.args["on"]);
  expect(talks).toEqual([1, 0]);
  await context.close();
});

test("pairing lands on the Remote page", async ({ browser }) => {
  const started = (await (await api("remote/pairing", { method: "POST" })).json()) as { code: string };
  const context = await phoneContext(browser);
  const page = await context.newPage();
  await page.goto(`${server.url}/pair#code=${started.code}`);
  await page.getByTestId("pair-name").fill("Pocket");
  await page.getByTestId("pair-submit").tap();
  await page.waitForURL(`${server.url}/#/remote`);
  await expect(page.locator("ga-remote")).toBeVisible();
  await expect(inRemote(page).getByTestId("remote-mix")).toBeVisible();
  // The plain address lands there too, on a phone.
  await page.goto(`${server.url}/`);
  await expect(page.locator("ga-remote")).toBeVisible();
  expect(new URL(page.url()).hash).toBe("#/remote");
  await context.close();
});

test("a revoked phone is told so, in place of the app, and stops trying", async ({ browser }) => {
  const { context, id } = await pairedPhone(browser);
  const page = await context.newPage();
  let sockets = 0;
  page.on("websocket", () => sockets++);
  await page.goto(`${server.url}/#/remote/${QUADRO}`);
  await expect(page.getByTestId("connection")).toHaveAttribute("data-state", "open");
  expect(sockets).toBe(1);

  expect((await api(`remote/phones/${id}`, { method: "DELETE" })).status).toBe(200);
  const told = page.getByTestId("unpaired");
  await expect(told).toBeVisible();
  await expect(told.getByRole("heading")).toHaveText("This phone is no longer paired with Gazelle");
  await expect(told).toContainText("Pair it again from the computer: Workspace, Phones.");
  await expect(told.getByRole("link", { name: "Pair again" })).toHaveAttribute("href", "/pair");
  // Nothing knocks again: the client would be back within a second or two if it were trying.
  await page.waitForTimeout(3000);
  expect(sockets).toBe(1);
  await context.close();
});

test("screenshots of the Remote page at 390 by 844, dark and light", async ({ browser }) => {
  test.skip(SHOTS === undefined, "set REMOTE_SCREENSHOTS to a directory to take them");
  test.setTimeout(120_000);
  // A server of its own that reports, so the meters move; the phone seams again.
  const main = server;
  server = await startServer(["--backend", "loopback", "--dry-run", "--loopback-cyclic-ms", "100"], { webUi: true, phoneSeams: true });
  try {
    await allowPhones(true);
    await layout();
    await shots(browser);
  } finally {
    await server.stop();
    server = main;
  }
});

async function shots(browser: Browser): Promise<void> {
  for (const [scheme, theme] of [["dark", "gazelle-dark"], ["light", "gazelle-light"]] as const) {
    for (const device of [QUADRO, STUDIO]) {
      const { context } = await pairedPhone(browser, scheme);
      await context.addInitScript((picked) => localStorage.setItem("gazelle.theme", JSON.stringify(picked).slice(1, -1)), theme);
      const page = await context.newPage();
      await page.goto(`${server.url}/#/remote/${device}`);
      await expect(inRemote(page).getByTestId("remote-mix").locator("ga-remote-strip").first()).toBeVisible();
      await page.waitForTimeout(1200);
      await page.screenshot({ path: `${SHOTS}/remote-${device === QUADRO ? "quadro" : "studio"}-${scheme}.png` });
      // The whole page, scrolled through the app's own scrolling area.
      const height = await page.evaluate(() => document.querySelector("ga-app")?.shadowRoot?.querySelector("main")?.scrollHeight ?? 844);
      await page.setViewportSize({ width: 390, height: Math.min(4000, height + 140) });
      await page.waitForTimeout(400);
      await page.screenshot({ path: `${SHOTS}/remote-${device === QUADRO ? "quadro" : "studio"}-${scheme}-full.png` });
      await context.close();
    }
  }
}
