// Moving between pages keeps your place (hands-on feedback items 2-4, decision P71): the device last
// opened follows you to pages whose address names none, each device keeps its selected mix, and a
// page left and come back to finds its scroll, open sections and selections as they were. The
// device and mix are remembered per browser; the rest for the tab.

import { expect, test, type Page, type WebSocketRoute } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

/** Follows a header link, as a person moving between pages does. */
const open = (page: Page, name: "devices" | "workspace" | "inputs" | "outputs" | "mixer" | "routing") => page.locator(`ga-header nav a[data-page="${name}"]`).click();

test("the device last opened follows you to every page, and across a reload", async ({ page }) => {
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(page.locator("ga-inputs").getByLabel("Device")).toHaveValue("loopback-1");

  await open(page, "outputs");
  await expect(page.locator("ga-outputs").getByLabel("Device")).toHaveValue("loopback-1");
  await open(page, "mixer");
  await expect(page.locator("ga-mixer").getByLabel("Device", { exact: true })).toHaveValue("loopback-1");
  await open(page, "routing");
  await expect(page.locator("ga-routing").getByLabel("Device")).toHaveValue("loopback-1");
  await open(page, "devices");
  await expect(page.locator("ga-device-status")).toHaveAttribute("device-id", "loopback-1");
  await expect(page.locator('ga-device-list a[data-device-id="loopback-1"]')).toHaveAttribute("aria-current", "page");
  // The Control Room panel shows it too, on a page without a device.
  await open(page, "workspace");
  await expect(page.locator("ga-monitor")).toHaveAttribute("device-id", "loopback-1");

  // A device picked on a page's own picker is the new one.
  await open(page, "outputs");
  await page.locator("ga-outputs").getByLabel("Device").selectOption("loopback-0");
  await open(page, "inputs");
  await expect(page.locator("ga-inputs").getByLabel("Device")).toHaveValue("loopback-0");

  // An old link to a device that is not connected does not replace it.
  await page.goto(`${server.url}/#/inputs/usb-gone`);
  await expect(page.locator("ga-inputs")).toContainText("not connected");
  await page.goto(`${server.url}/#/routing`);
  await expect(page.locator("ga-routing").getByLabel("Device")).toHaveValue("loopback-0");

  await page.getByLabel("Pages").locator('a[data-page="devices"]').click();
  await page.locator('ga-device-list a[data-device-id="loopback-1"]').click();
  await page.reload();
  await open(page, "outputs");
  await expect(page.locator("ga-outputs").getByLabel("Device")).toHaveValue("loopback-1");
});

test("each device keeps its selected mix: the address names it, and a page without one uses the last", async ({ page }) => {
  // A deep link still opens its mix.
  await page.goto(`${server.url}/#/mixer/loopback-0/2`);
  const metered = page.getByTestId("metered-mix");
  await expect(metered).toHaveValue("2");

  // Choosing a mix puts it in the address without rebuilding the page.
  await page.locator("ga-mixer").evaluate((el) => ((el as unknown as { __kept: boolean }).__kept = true));
  await metered.selectOption("1");
  await expect(page).toHaveURL(/#\/mixer\/loopback-0\/1$/);
  expect(await page.locator("ga-mixer").evaluate((el) => (el as unknown as { __kept?: boolean }).__kept)).toBe(true);

  await page.locator("ga-mixer").getByLabel("Device", { exact: true }).selectOption("loopback-1");
  await expect(metered).toHaveValue("0");
  await metered.selectOption("3");

  await open(page, "inputs");
  await open(page, "mixer");
  await expect(page.locator("ga-mixer").getByLabel("Device", { exact: true })).toHaveValue("loopback-1");
  await expect(metered).toHaveValue("3");
  await page.locator("ga-mixer").getByLabel("Device", { exact: true }).selectOption("loopback-0");
  await expect(metered).toHaveValue("1");

  await page.goto(`${server.url}/#/mixer`);
  await page.reload();
  await expect(page.locator("ga-mixer").getByLabel("Device", { exact: true })).toHaveValue("loopback-0");
  await expect(metered).toHaveValue("1");
});

test("a page that is not shown is gone and sends nothing; choosing a mix or coming back re-reads no mix", async ({ page }) => {
  // View state is kept in the store rather than by keeping pages alive, so leaving a page must
  // dispose of it: no element left to follow reports or point the device's meters.
  const sent: string[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") {
        const command = (JSON.parse(event.payload) as { command?: string }).command;
        if (command !== undefined) sent.push(command);
      }
    }),
  );
  const count = (command: string) => sent.filter((c) => c === command).length;
  const inApp = (tag: string) => page.locator("ga-app").evaluate((app, t) => app.shadowRoot?.querySelectorAll(t).length ?? -1, tag);

  await page.goto(`${server.url}/#/mixer/loopback-1`);
  await expect.poll(() => count("get_mixer")).toBe(4);
  await expect.poll(() => count("set_peak_source")).toBe(1);

  await page.getByTestId("metered-mix").selectOption("2");
  await expect.poll(() => count("set_peak_source")).toBe(2);
  await page.waitForTimeout(500);
  expect(count("get_mixer"), "a new mix does not rebuild the page and read every mix again").toBe(4);

  await open(page, "workspace");
  await expect.poll(() => inApp("ga-mixer")).toBe(0);
  const before = sent.length;
  await page.waitForTimeout(1000);
  expect(sent.slice(before)).toEqual([]);

  // Coming back reads no mix, nor the device's link flags that follow a read (P80): the store has
  // them from the first visit. The meters are pointed at the page's mix again, and where each mix
  // plays is read again (routing is read fresh by design, P80).
  await open(page, "mixer");
  await expect.poll(() => count("set_peak_source")).toBe(3);
  await page.waitForTimeout(1000);
  expect(sent.slice(before).filter((c) => c.startsWith("get_") && c !== "get_routing"), "a second visit re-reads no mix").toEqual([]);
});

test("the Mixer reads every mix again when the connection to the server comes back", async ({ page }) => {
  const sent: string[] = [];
  const sockets: WebSocketRoute[] = [];
  await page.routeWebSocket(/\/ws$/, (ws) => {
    const upstream = ws.connectToServer();
    sockets.push(ws);
    ws.onMessage((message) => {
      if (typeof message === "string") {
        const command = (JSON.parse(message) as { command?: string }).command;
        if (command !== undefined) sent.push(command);
      }
      upstream.send(message);
    });
  });
  const count = (command: string) => sent.filter((c) => c === command).length;

  await page.goto(`${server.url}/#/mixer/loopback-1`);
  await expect.poll(() => count("get_mixer")).toBe(4);
  await open(page, "inputs");
  await open(page, "mixer");
  await page.waitForTimeout(500);
  expect(count("get_mixer"), "not on coming back").toBe(4);

  // The server may have restarted, or the device been changed, while the connection was down.
  await sockets.at(-1)?.close();
  await expect(page.getByTestId("connection")).toHaveText("Reconnecting…");
  await expect(page.getByTestId("connection")).toHaveText("Connected", { timeout: 15_000 });
  await expect.poll(() => count("get_mixer"), "while the page is open").toBe(8);
  await expect(page.getByText(/could not be read/), "and not tried while the connection was down").toHaveCount(0);
  await open(page, "inputs");
  await open(page, "mixer");
  await page.waitForTimeout(500);
  expect(count("get_mixer"), "and once only").toBe(8);
});

test("a half-typed name is a draft: leaving the page keeps it without saving it, until Enter or Escape", async ({ page }) => {
  const saved = async () => (await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { aliases: Record<string, string>; mixers: Record<string, { mixes: { name: string }[]; groups: { name: string }[] }> };
  const settle = () => page.waitForTimeout(700); // longer than the workspace's save debounce
  await putWorkspace(server, {
    mixers: {
      "loopback-0": {
        mixes: [{ name: "Monitors" }],
        groups: [{ id: "g", name: "Drums", collapsed: false }],
        channels: [{ id: "a", name: "Kick", slot: 6, group: "g", source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }],
      },
    },
  });

  // Devices: typed, then a header link. Not saved, and there again on coming back, for that device only.
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const deviceName = page.getByTestId("device-name");
  await deviceName.fill("Desk Qu");
  await open(page, "workspace");
  await expect(page.getByLabel("Name for loopback-0")).toHaveValue("");
  await settle();
  expect((await saved()).aliases["loopback-0"]).toBeUndefined();
  await open(page, "devices");
  await expect(deviceName).toHaveValue("Desk Qu");
  await page.locator('ga-device-list a[data-device-id="loopback-1"]').click();
  await expect(page.locator("ga-device-status")).toHaveAttribute("device-id", "loopback-1");
  await expect(deviceName).toHaveValue("");
  await page.locator('ga-device-list a[data-device-id="loopback-0"]').click();
  await expect(deviceName).toHaveValue("Desk Qu");
  // Escape drops the draft.
  await deviceName.press("Escape");
  await expect(deviceName).toHaveValue("");
  await open(page, "workspace");
  await open(page, "devices");
  await expect(deviceName).toHaveValue("");

  // Workspace: left without the field losing focus first (the address changed), then Enter saves it.
  await open(page, "workspace");
  const studioName = page.getByLabel("Name for loopback-1");
  await studioName.fill("Stud");
  await page.evaluate(() => (location.hash = "#/routing"));
  await expect(page.locator("ga-routing")).toBeVisible();
  await open(page, "workspace");
  await expect(studioName).toHaveValue("Stud");
  await settle();
  expect((await saved()).aliases["loopback-1"]).toBeUndefined();
  await studioName.press("Enter");
  await expect.poll(async () => (await saved()).aliases["loopback-1"]).toBe("Stud");
  // Committed, it is no longer a draft: a new name given elsewhere shows here.
  await open(page, "devices");
  await page.locator('ga-device-list a[data-device-id="loopback-1"]').click();
  await deviceName.fill("Studio");
  await deviceName.press("Enter");
  await open(page, "workspace");
  await expect(studioName).toHaveValue("Studio");

  // Mixer: a mix name kept over a visit elsewhere, then saved with Enter.
  await open(page, "mixer");
  await page.locator("ga-mixer").getByLabel("Device", { exact: true }).selectOption("loopback-0");
  const mixName = page.getByTestId("mix-name-0");
  await mixName.fill("Control Ro");
  await open(page, "outputs");
  await open(page, "mixer");
  await expect(mixName).toHaveValue("Control Ro");
  await settle();
  expect((await saved()).mixers["loopback-0"]?.mixes[0]?.name).toBe("Monitors");
  await mixName.press("Enter");
  await expect.poll(async () => (await saved()).mixers["loopback-0"]?.mixes[0]?.name).toBe("Control Ro");

  // A group name kept likewise; going into the field and leaving it for another on the page saves it.
  const groupName = page.getByRole("textbox", { name: "Group name" });
  await groupName.fill("Perc");
  await open(page, "routing");
  await open(page, "mixer");
  await expect(groupName).toHaveValue("Perc");
  await settle();
  expect((await saved()).mixers["loopback-0"]?.groups[0]?.name).toBe("Drums");
  await groupName.click();
  await page.getByTestId("layout-save-name").click();
  await expect.poll(async () => (await saved()).mixers["loopback-0"]?.groups[0]?.name).toBe("Perc");
  await open(page, "routing");
  await open(page, "mixer");
  await expect(groupName).toHaveValue("Perc");

  // After following a link here, leaving a field from the keyboard still saves it.
  const channelName = page.getByTestId("name-6");
  await channelName.fill("Kick In");
  await channelName.press("Tab");
  await expect.poll(async () => ((await saved()).mixers["loopback-0"] as unknown as { channels: { name: string }[] }).channels[0]?.name).toBe("Kick In");
});

test("a page left and come back to finds its scroll, sections and selections as they were", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 600 });
  const main = page.locator("ga-app main");

  // Devices: a collapsed section and the scroll position.
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const clock = page.locator('ga-device-status ga-section[heading="Clock"]');
  await clock.getByRole("button", { name: "Clock" }).click();
  await expect(clock).toHaveJSProperty("collapsed", true);
  await main.evaluate((el) => (el.scrollTop = 200));
  await expect.poll(() => main.evaluate((el) => el.scrollTop)).toBe(200);
  await page.waitForTimeout(100);
  await open(page, "workspace");
  await expect.poll(() => main.evaluate((el) => el.scrollTop)).toBe(0);
  await open(page, "devices");
  await expect(clock).toHaveJSProperty("collapsed", true);
  await expect.poll(() => main.evaluate((el) => el.scrollTop)).toBe(200);

  // Routing: the sources picked for a fill.
  await open(page, "routing");
  await page.getByTestId("source-0-0").click();
  await page.getByTestId("source-0-2").click({ modifiers: ["Shift"] });
  await open(page, "inputs");
  await open(page, "routing");
  await expect(page.getByTestId("source-0-1")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("source-0-3")).toHaveAttribute("aria-pressed", "false");

  // Mixer, before any channel is set up: the starting layout picked but not yet applied.
  await open(page, "mixer");
  await page.getByTestId("profile-select").selectOption("podcast");
  await open(page, "routing");
  await open(page, "mixer");
  await expect(page.getByTestId("profile-select")).toHaveValue("podcast");

  // Mixer: the notes, the channels' horizontal scroll, and a half-typed layout name.
  const channels = Array.from({ length: 26 }, (_, i) => (i === 0 ? { id: "c0", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] } : { id: `c${i}`, name: "", slot: 6 + i, sends: [] }));
  await putWorkspace(server, { mixers: { "loopback-0": { channels } } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await page.reload();
  const strips = page.locator("ga-mixer .strips");
  await expect(page.getByTestId("fader-6")).toBeVisible();
  await page.locator("ga-mixer details.notes summary").click();
  await strips.evaluate((el) => (el.scrollLeft = 150));
  await expect.poll(() => strips.evaluate((el) => el.scrollLeft)).toBe(150);
  await page.getByTestId("layout-save-name").fill("Half a na");
  await page.waitForTimeout(100);
  await open(page, "outputs");
  await open(page, "mixer");
  await expect(page.locator("ga-mixer details.notes")).toHaveJSProperty("open", true);
  await expect.poll(() => strips.evaluate((el) => el.scrollLeft)).toBe(150);
  await expect(page.getByTestId("layout-save-name")).toHaveValue("Half a na");
});
