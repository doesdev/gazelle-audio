// The Aggregate page. Nothing here reaches a driver, an aggregate or a DAW: the server is the
// loopback, and `GET /api/v1/aggregate` is answered in the browser with the shapes the real route
// gives, the way `driver.spec.ts` answers the driver's routes. Every request the page makes back to
// the server is captured and checked exactly, because a fix button whose body is nearly right is a
// change made to the wrong thing.

import { expect, test, type Page, type Route } from "@playwright/test";

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

const DLL = "C:\\Program Files\\Gazelle\\gazelle_aggregate.dll";

const registration = (parts: Record<string, unknown> = {}) => ({
  registered: true,
  clsid: "{7A1B0C4E-0000-0000-0000-000000000001}",
  name: "Gazelle Aggregate",
  dll: DLL,
  dll_present: true,
  message: `Gazelle Aggregate is registered, pointing at ${DLL}.`,
  register_command: `regsvr32 /s "${DLL}"`,
  unregister_command: `regsvr32 /s /u "${DLL}"`,
  dll_search: { state: "found", dll: DLL },
  ...parts,
});

const deviceReport = (name: string, parts: Record<string, unknown> = {}) => ({
  name,
  key: name,
  registered: true,
  entry_key: name,
  device_id: "loopback-0",
  attached: true,
  family: "quadro",
  clock: { source_index: 0, source: "Internal", locked: true, hz: 96000, rate_index: 4 },
  driver: { sample_rate: 96000, buffer_size: 256, safe_mode: false, asio_clients: 0 },
  is_master: false,
  ...parts,
});

const liveDevice = (name: string, parts: Record<string, unknown> = {}) => ({
  name,
  driver_name: name,
  streaming: true,
  stalled: false,
  is_master: false,
  sample_gap: 0,
  callbacks: 1200,
  dropped: 0,
  starved: 0,
  ...parts,
});

const answer = (parts: Record<string, unknown> = {}) => ({
  read_at_ms: Date.UTC(2026, 8, 21, 10, 0, 0),
  configured: true,
  export_path: "C:\\Users\\someone\\AppData\\Roaming\\gazelle\\aggregate.json",
  drivers: [
    { key: "Zen Quadro Synergy Core", description: "Zen Quadro Synergy Core", clsid: "{1}", dll: "q.dll", dll_present: true, configured: true, is_aggregate: false },
    { key: "Zen Studio+", description: "Zen Studio+", clsid: "{2}", dll: "s.dll", dll_present: true, configured: false, is_aggregate: false },
    { key: "Gazelle Aggregate", description: "Gazelle Aggregate", clsid: "{3}", dll: DLL, dll_present: true, configured: false, is_aggregate: true },
  ],
  registration: registration(),
  devices: [],
  ready: true,
  reasons: [],
  status: { state: "silent", message: "Gazelle Aggregate is not loaded in any program, so there is nothing live to report." },
  events: [],
  ...parts,
});

interface Captured {
  /** Every POST the page made, as route and body, in order. */
  posts: { route: string; body: unknown }[];
  /** Every command the page sent a device, which goes over the event socket rather than over HTTP. */
  commands: { device_id: string; command: string; args?: unknown }[];
  /** How many times the page read the whole answer. */
  reads: number;
}

/**
 * Answers `GET /aggregate` with `reading` (or the next of several, so a fix can change what comes
 * back) and captures every POST the page makes to an aggregate or command route without letting it
 * reach the server.
 */
async function fakeAggregate(page: Page, ...readings: Record<string, unknown>[]): Promise<Captured> {
  const captured: Captured = { posts: [], commands: [], reads: 0 };
  // A device command is a frame on the event socket. It still reaches this loopback server, which
  // is in dry run, so nothing is written anywhere; what is checked is the frame the page sent.
  page.on("websocket", (socket) =>
    socket.on("framesent", ({ payload }) => {
      const frame = JSON.parse(String(payload)) as { device_id?: string; command?: string; args?: unknown };
      // Reads are left out: the shell reads every device's state the whole time, and what is being
      // checked here is what this page writes.
      if (frame.device_id !== undefined && frame.command !== undefined && !frame.command.startsWith("get_")) {
        captured.commands.push({ device_id: frame.device_id, command: frame.command, ...(frame.args === undefined ? {} : { args: frame.args }) });
      }
    }),
  );
  await page.route("**/api/v1/aggregate**", async (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace("/api/v1/", "");
    if (request.method() === "POST") {
      captured.posts.push({ route: path, body: request.postDataJSON() });
      if (path.endsWith("match-buffers")) return route.fulfill({ json: { buffer_size: (request.postDataJSON() as { buffer_size: number }).buffer_size, changed: 2, refused: 0, devices: [] } });
      return route.fulfill({ json: { dll: DLL, command: "regsvr32", run: { started: true, exit_code: 0, message: "The driver was registered." } } });
    }
    const at = Math.min(captured.reads, readings.length - 1);
    captured.reads += 1;
    await route.fulfill({ json: readings[at] });
  });
  return captured;
}

const open = async (page: Page) => {
  await page.goto(`${server.url}/#/aggregate`);
  await expect(page.locator("ga-aggregate")).toBeVisible();
};

// ---------------------------------------------------------------------------------------------
// Nothing set up
// ---------------------------------------------------------------------------------------------

test("with nothing set up the page says so, offers interfaces to add, and shows no live figures", async ({ page }) => {
  await fakeAggregate(
    page,
    answer({
      configured: false,
      ready: false,
      reasons: [{ code: "not_configured", severity: "blocking", message: "No interfaces have been chosen for the aggregate yet. Add the ones it should open, in the order their channels should appear." }],
    }),
  );
  await open(page);
  await expect(page.getByTestId("aggregate-verdict")).toHaveText("Not ready");
  await expect(page.getByTestId("reason-not_configured")).toContainText("No interfaces have been chosen");
  await expect(page.getByTestId("reason-severity-not_configured")).toHaveText("STOPS IT");
  await expect(page.getByTestId("reason-fix-not_configured")).toHaveCount(0);
  await expect(page.getByTestId("aggregate-no-devices")).toBeVisible();
  // The aggregate's own driver is never offered as something to put inside itself.
  await expect(page.getByTestId("aggregate-add-select").locator("option")).toHaveText(["Zen Quadro Synergy Core", "Zen Studio+"]);
  await expect(page.getByTestId("aggregate-live-note")).toContainText("not loaded in any program");
  await expect(page.getByTestId("aggregate-events-note")).toContainText("written nothing yet");
});

test("adding an interface writes it into the workspace and gives it a card", async ({ page }) => {
  await fakeAggregate(page, answer({ configured: false, ready: false, reasons: [] }));
  await open(page);
  await page.getByTestId("aggregate-add-select").selectOption("Zen Studio+");
  await page.getByTestId("aggregate-add").click();
  await expect(page.getByTestId("aggregate-device-0")).toBeVisible();
  await expect(page.getByTestId("device-0-entry")).toHaveText("Zen Studio+");
  await expect.poll(async () => (await (await fetch(`${server.url}/api/v1/workspace`)).json()).aggregate?.devices).toEqual([{ key: "Zen Studio+", name: "Zen Studio+" }]);
});

// ---------------------------------------------------------------------------------------------
// The server does not offer it at all
// ---------------------------------------------------------------------------------------------

test("a server that does not serve the aggregate routes says so instead of failing", async ({ page }) => {
  await page.route("**/api/v1/aggregate**", (route) => route.fulfill({ status: 404, body: "" }));
  await open(page);
  await expect(page.getByTestId("aggregate-unavailable")).toBeVisible();
  await expect(page.getByTestId("aggregate-sections")).toBeHidden();
  await expect(page.getByTestId("aggregate-verdict")).toBeHidden();
});

// ---------------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------------

test("a driver that is not registered says so, shows the command, and its button registers it", async ({ page }) => {
  const captured = await fakeAggregate(
    page,
    answer({
      ready: false,
      registration: registration({ registered: false, dll: undefined, dll_present: false, message: "Gazelle Aggregate is not registered on this PC." }),
      reasons: [
        {
          code: "not_registered",
          severity: "blocking",
          message: "Gazelle Aggregate is not registered on this PC, so no DAW can choose it. Registering needs administrator rights.",
          fix: { kind: "register", method: "POST", route: "aggregate/register", body: {}, label: "Register the driver" },
        },
      ],
    }),
    answer(),
  );
  await open(page);
  await expect(page.getByTestId("aggregate-registered")).toHaveText("No");
  await expect(page.getByTestId("aggregate-registered-dll")).toHaveText("Nothing: it is not registered");
  await expect(page.getByTestId("aggregate-found-dll")).toHaveText(DLL);
  await expect(page.getByTestId("aggregate-command")).toHaveText(`regsvr32 /s "${DLL}"`);
  await expect(page.getByTestId("aggregate-admin-note")).toContainText("the one thing in Gazelle that asks for administrator rights");
  await expect(page.getByTestId("aggregate-unregister")).toBeHidden();

  // The reason's own button sends the request the answer named, and nothing else.
  await page.getByTestId("reason-fix-not_registered").click();
  await expect.poll(() => captured.posts).toEqual([{ route: "aggregate/register", body: {} }]);
  await expect(page.getByTestId("aggregate-outcome")).toHaveText("Registering finished. The driver was registered.");
  // And afterwards the whole answer is read again, so the page shows what the server now says:
  // the reason goes, and with it the last thing in the way.
  await expect(page.getByTestId("aggregate-registered")).toHaveText("Yes");
  await expect(page.getByTestId("reason-row-not_registered")).toHaveCount(0);
  await expect(page.getByTestId("aggregate-verdict")).toHaveText("Ready");
  await expect(page.getByTestId("aggregate-no-reasons")).toBeVisible();
});

test("a registration pointing at a copy that has gone says so and offers to register it again", async ({ page }) => {
  await fakeAggregate(
    page,
    answer({
      ready: false,
      registration: registration({ dll: "C:\\Old\\gazelle_aggregate.dll", dll_present: false }),
      reasons: [
        {
          code: "dll_missing",
          severity: "blocking",
          message: "Gazelle Aggregate is registered, and the copy its registration points at is not there any more. Register it again from where the file is now.",
          fix: { kind: "register", method: "POST", route: "aggregate/register", body: {}, label: "Register the driver" },
        },
      ],
    }),
  );
  await open(page);
  await expect(page.getByTestId("aggregate-dll-present")).toHaveText("No, so a DAW opening it would fail");
  await expect(page.getByTestId("aggregate-registered-dll")).toHaveText("C:\\Old\\gazelle_aggregate.dll");
  await expect(page.getByTestId("aggregate-register")).toHaveText("Register it again");
  await expect(page.getByTestId("aggregate-command")).toHaveText(`regsvr32 /s /u "${DLL}"`);
});

// ---------------------------------------------------------------------------------------------
// The reasons and their fixes
// ---------------------------------------------------------------------------------------------

test("a rate and a clock fix each send exactly the command the server prepared", async ({ page }) => {
  const captured = await fakeAggregate(
    page,
    answer({
      ready: false,
      devices: [deviceReport("Quadro", { is_master: true }), deviceReport("Studio+", { clock: { source_index: 0, source: "Internal", locked: false, hz: 48000, rate_index: 2 } })],
      reasons: [
        {
          code: "rates_differ",
          severity: "blocking",
          message: "Studio+ is at 48 kHz and the others are at 96 kHz. Every interface in the aggregate has to be at one rate.",
          device: "Studio+",
          device_id: "loopback-0",
          fix: { kind: "set_sample_rate", method: "POST", route: "devices/loopback-0/command/set_samp_rate", body: { srate_idx: 4 }, label: "Put Studio+ at 96 kHz" },
        },
        {
          code: "clock_not_cabled",
          severity: "blocking",
          message: "Studio+ says it is clocked from Internal, and its cable arrives on S/PDIF. It will be USB clocked the moment a DAW opens it, and then it drifts.",
          device: "Studio+",
          device_id: "loopback-0",
          fix: { kind: "set_clock_source", method: "POST", route: "devices/loopback-0/command/set_sync_source", body: { src_index: 5 }, label: "Put Studio+ on S/PDIF" },
        },
      ],
    }),
  );
  await open(page);
  await expect(page.getByTestId("reason-fix-rates_differ")).toHaveText("Put Studio+ at 96 kHz");
  await page.getByTestId("reason-fix-rates_differ").click();
  await expect.poll(() => captured.commands).toEqual([{ device_id: "loopback-0", command: "set_samp_rate", args: { srate_idx: 4 } }]);

  await page.getByTestId("reason-fix-clock_not_cabled").click();
  await expect.poll(() => captured.commands).toEqual([
    { device_id: "loopback-0", command: "set_samp_rate", args: { srate_idx: 4 } },
    { device_id: "loopback-0", command: "set_sync_source", args: { src_index: 5 } },
  ]);
  expect(captured.posts, "a device command is not an aggregate route").toEqual([]);
});

test("a warning is marked as one, and the page is still ready", async ({ page }) => {
  await fakeAggregate(
    page,
    answer({
      ready: true,
      reasons: [{ code: "controller_unknown", severity: "warning", message: "Quadro's USB host controller could not be found, so Gazelle cannot tell whether the two interfaces share one." }],
    }),
  );
  await open(page);
  await expect(page.getByTestId("aggregate-verdict")).toHaveText("Ready");
  await expect(page.getByTestId("reason-severity-controller_unknown")).toHaveText("WORTH KNOWING");
});

test("nothing in the way says so in as many words", async ({ page }) => {
  await fakeAggregate(page, answer({ ready: true, reasons: [] }));
  await open(page);
  await expect(page.getByTestId("aggregate-verdict")).toHaveText("Ready");
  await expect(page.getByTestId("aggregate-no-reasons")).toBeVisible();
  await expect(page.getByTestId("aggregate-reasons")).toBeHidden();
});

// ---------------------------------------------------------------------------------------------
// Matching buffers, which interrupts a DAW
// ---------------------------------------------------------------------------------------------

test("matching buffer sizes asks twice, then sends the master's size and reads again", async ({ page }) => {
  const captured = await fakeAggregate(
    page,
    answer({
      ready: false,
      devices: [deviceReport("Quadro", { is_master: true }), deviceReport("Studio+", { driver: { buffer_size: 128, safe_mode: false, asio_clients: 0 } })],
      reasons: [
        {
          code: "buffers_differ",
          severity: "blocking",
          message: "The interfaces' drivers are on different buffer sizes (256 and 128). They will not run together until they match.",
          fix: { kind: "match_buffers", method: "POST", route: "aggregate/match-buffers", body: { buffer_size: 256 }, label: "Put them all on 256 samples" },
        },
      ],
    }),
  );
  await open(page);
  const fix = page.getByTestId("reason-fix-buffers_differ");
  await expect(fix).toHaveText("Put them all on 256 samples");
  await expect(fix).toHaveAttribute("title", /restarts its audio/);

  // One click arms it and sends nothing.
  await fix.click();
  await expect(fix).toHaveText("Confirm");
  await expect(fix).toHaveAttribute("data-armed", "");
  await page.waitForTimeout(300);
  expect(captured.posts).toEqual([]);

  // The second sends exactly what the server prepared.
  await fix.click();
  await expect.poll(() => captured.posts).toEqual([{ route: "aggregate/match-buffers", body: { buffer_size: 256 } }]);
  await expect(page.getByTestId("aggregate-outcome")).toHaveText("2 devices put on 256 samples.");
});

test("a match left unconfirmed forgets itself and sends nothing", async ({ page }) => {
  const captured = await fakeAggregate(
    page,
    answer({
      ready: false,
      devices: [deviceReport("Quadro", { is_master: true })],
      reasons: [
        {
          code: "buffers_differ",
          severity: "blocking",
          message: "The interfaces' drivers are on different buffer sizes.",
          fix: { kind: "match_buffers", method: "POST", route: "aggregate/match-buffers", body: { buffer_size: 256 }, label: "Put them all on 256 samples" },
        },
      ],
    }),
  );
  await open(page);
  await page.getByTestId("reason-fix-buffers_differ").click();
  await expect(page.getByTestId("reason-fix-buffers_differ")).toHaveText("Confirm");
  await expect(page.getByTestId("reason-fix-buffers_differ")).toHaveText("Put them all on 256 samples", { timeout: 6000 });
  expect(captured.posts).toEqual([]);
});

test("the Match buffer sizes button in the interfaces section asks twice as well", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0" }, { key: "Studio+", name: "Studio+" }] } });
  const captured = await fakeAggregate(page, answer({ devices: [deviceReport("Quadro", { is_master: true }), deviceReport("Studio+")] }));
  await open(page);
  const match = page.getByTestId("aggregate-match");
  await match.click();
  await expect(match).toHaveText("Confirm");
  await match.click();
  await expect.poll(() => captured.posts).toEqual([{ route: "aggregate/match-buffers", body: { buffer_size: 256 } }]);
});

// ---------------------------------------------------------------------------------------------
// The interfaces, and editing them
// ---------------------------------------------------------------------------------------------

test("each interface shows what it is doing, and its buffer size is sent only from its Confirm", async ({ page }) => {
  await putWorkspace(server, {
    aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0", input_trim: 12 }, { key: "Studio+", name: "Studio+" }], callback_master: "Quadro", alignment: "aligned" },
  });
  await fakeAggregate(page, answer({ devices: [deviceReport("Quadro", { is_master: true }), deviceReport("Studio+", { attached: false, device_id: undefined, clock: undefined, driver: { message: "Not connected" } })] }));
  await open(page);

  await expect(page.getByTestId("device-0-name")).toHaveValue("Quadro");
  await expect(page.getByTestId("device-0-clock")).toHaveText("Internal");
  await expect(page.getByTestId("device-0-lock")).toHaveText("LOCK");
  await expect(page.getByTestId("device-0-rate")).toHaveText("96 kHz");
  await expect(page.getByTestId("device-0-channels")).toHaveText("All in, All out");
  await expect(page.getByTestId("device-0-in-trim")).toHaveValue("12");
  await expect(page.getByTestId("device-0-gap")).toHaveText("No DAW has it open");
  await expect(page.getByTestId("device-0-buffer")).toHaveValue("256");
  // The one that is not connected reads as not known rather than as a guess.
  await expect(page.getByTestId("device-1-clock")).toHaveText("Not connected");
  await expect(page.getByTestId("device-1-lock")).toHaveText("NO LOCK");

  await expect(page.getByTestId("aggregate-master")).toHaveValue("Quadro");
  await expect(page.getByTestId("aggregate-alignment")).toHaveValue("aligned");
  await expect(page.getByTestId("aggregate-export-path")).toContainText("aggregate.json");

  // Choosing a buffer size shows a Confirm and sends nothing until it is pressed.
  const puts: unknown[] = [];
  await page.route("**/api/v1/devices/*/driver", async (route) => {
    if (route.request().method() !== "PUT") return route.fallback();
    puts.push(route.request().postDataJSON());
    await route.fulfill({ status: 409, json: { error: { code: "asio_in_use", message: "in use" } } });
  });
  await page.getByTestId("device-0-buffer").selectOption("128");
  await expect(page.getByTestId("device-0-buffer-confirm")).toBeVisible();
  await page.waitForTimeout(200);
  expect(puts).toEqual([]);
  await page.getByTestId("device-0-buffer-confirm").click();
  await expect.poll(() => puts).toEqual([{ buffer_size: 128 }]);
});

test("the order of the interfaces is the order of the channels, and it can be changed", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro" }, { key: "Studio+", name: "Studio+" }] } });
  await fakeAggregate(page, answer({ devices: [deviceReport("Quadro"), deviceReport("Studio+")] }));
  await open(page);
  await expect(page.getByTestId("device-0-name")).toHaveValue("Quadro");
  await expect(page.getByTestId("device-0-up")).toBeDisabled();

  await page.getByTestId("device-1-up").click();
  await expect(page.getByTestId("device-0-name")).toHaveValue("Studio+");
  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()).aggregate?.devices ?? []).map((d: { name: string }) => d.name)).toEqual(["Studio+", "Quadro"]);

  // Removing takes two clicks, as everything that throws work away here does.
  await page.getByTestId("device-0-remove").click();
  await expect(page.getByTestId("device-0-remove")).toHaveText("Confirm");
  await page.getByTestId("device-0-remove").click();
  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()).aggregate?.devices ?? []).map((d: { name: string }) => d.name)).toEqual(["Quadro"]);
});

// ---------------------------------------------------------------------------------------------
// Live, while a DAW has it open
// ---------------------------------------------------------------------------------------------

const streaming = (devices: Record<string, unknown>[], parts: Record<string, unknown> = {}) => ({
  state: "read",
  open: true,
  streaming: true,
  generation: 2,
  generation_in_force: 2,
  up_to_date: true,
  plan: { master: "Quadro", rate: 96000, buffer_size: 256, inputs: 40, outputs: 40, alignment: "aligned", input_latency: 611, output_latency: 733 },
  devices,
  ...parts,
});

test("while a DAW is streaming the plan and every device's gap are on screen, and zero reads as in step", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro" }, { key: "Studio+", name: "Studio+" }] } });
  await fakeAggregate(
    page,
    answer({
      devices: [deviceReport("Quadro", { is_master: true }), deviceReport("Studio+")],
      status: streaming([liveDevice("Quadro", { is_master: true }), liveDevice("Studio+", { sample_gap: 0 })]),
    }),
  );
  await open(page);
  await expect(page.getByTestId("aggregate-live-note")).toHaveText("A DAW has the aggregate open and audio is running.");
  await expect(page.getByTestId("plan-master")).toHaveText("Quadro");
  await expect(page.getByTestId("plan-rate")).toHaveText("96 kHz");
  await expect(page.getByTestId("plan-buffer")).toHaveText("256 samples");
  await expect(page.getByTestId("plan-channels")).toHaveText("40 in, 40 out");
  await expect(page.getByTestId("plan-latency")).toHaveText("611 in, 733 out, in samples");
  await expect(page.getByTestId("live-gap-Quadro")).toHaveText("Master");
  await expect(page.getByTestId("live-gap-Studio+")).toHaveText("In step");
  await expect(page.getByTestId("live-gap-Studio+")).toHaveAttribute("data-tone", "good");
  await expect(page.getByTestId("device-1-gap")).toHaveText("In step");
});

test("a gap that is not zero says which way it is off and is marked as a problem", async ({ page }) => {
  await fakeAggregate(
    page,
    answer({
      devices: [deviceReport("Quadro", { is_master: true }), deviceReport("Studio+")],
      status: streaming([liveDevice("Quadro", { is_master: true }), liveDevice("Studio+", { sample_gap: -1920, starved: 3 })]),
    }),
  );
  await open(page);
  await expect(page.getByTestId("live-gap-Studio+")).toHaveText("1920 samples behind");
  await expect(page.getByTestId("live-gap-Studio+")).toHaveAttribute("data-tone", "off");
  await expect(page.getByTestId("live-starved-Studio+")).toHaveText("3 starved");
});

test("a stalled device is said as a stall, whatever its counters say", async ({ page }) => {
  await fakeAggregate(
    page,
    answer({
      devices: [deviceReport("Quadro", { is_master: true }), deviceReport("Studio+")],
      status: streaming([liveDevice("Quadro", { is_master: true }), liveDevice("Studio+", { stalled: true, streaming: false, sample_gap: 0 })]),
      events: [
        { at: "2026-09-21 09:14:02", kind: "session-started", message: "40 in, 40 out at 96000 Hz" },
        { at: "2026-09-21 09:31:44", kind: "stalled", message: "Studio+" },
      ],
    }),
  );
  await open(page);
  await expect(page.getByTestId("live-gap-Studio+")).toHaveText("Stalled");
  await expect(page.getByTestId("live-gap-Studio+")).toHaveAttribute("data-tone", "stalled");
  await expect(page.getByTestId("aggregate-event")).toHaveCount(2);
  await expect(page.getByTestId("aggregate-event").last()).toContainText("stalled");
  await expect(page.getByTestId("aggregate-events-note")).toContainText("still says why today");
});

test("a driver running an older setup than the one asked for says so, with what it last refused", async ({ page }) => {
  await fakeAggregate(
    page,
    answer({
      devices: [deviceReport("Quadro", { is_master: true })],
      status: streaming([liveDevice("Quadro", { is_master: true })], {
        generation: 5,
        generation_in_force: 4,
        up_to_date: false,
        last_refusal: "Studio+ will not run at 96000 Hz, so neither will the aggregate",
      }),
    }),
  );
  await open(page);
  await expect(page.getByTestId("plan-generation")).toContainText("Generation 4");
  await expect(page.getByTestId("plan-refusal")).toHaveText("Studio+ will not run at 96000 Hz, so neither will the aggregate");
});

test("the answer is read again while a DAW is streaming, and not once the page has gone", async ({ page }) => {
  const captured = await fakeAggregate(page, answer({ devices: [deviceReport("Quadro", { is_master: true })], status: streaming([liveDevice("Quadro", { is_master: true })]) }));
  await open(page);
  await expect.poll(() => captured.reads, { timeout: 8000 }).toBeGreaterThan(2);

  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(page.locator("ga-device-status")).toBeVisible();
  const settled = captured.reads;
  await page.waitForTimeout(3000);
  expect(captured.reads, "a page that is not shown asks for nothing").toBeLessThanOrEqual(settled + 1);
});

// ---------------------------------------------------------------------------------------------
// Phone width
// ---------------------------------------------------------------------------------------------

test("at phone width the page fits, with nothing running off the side", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0" }, { key: "Studio+", name: "Studio+" }] } });
  await fakeAggregate(
    page,
    answer({
      ready: false,
      devices: [deviceReport("Quadro", { is_master: true }), deviceReport("Studio+")],
      status: streaming([liveDevice("Quadro", { is_master: true }), liveDevice("Studio+", { sample_gap: 64 })]),
      reasons: [
        {
          code: "buffers_differ",
          severity: "blocking",
          message: "The interfaces' drivers are on different buffer sizes (256 and 128). They will not run together until they match.",
          fix: { kind: "match_buffers", method: "POST", route: "aggregate/match-buffers", body: { buffer_size: 256 }, label: "Put them all on 256 samples" },
        },
      ],
    }),
  );
  await page.setViewportSize({ width: 375, height: 812 });
  await open(page);
  await expect(page.getByTestId("aggregate-verdict")).toBeVisible();
  await expect(page.getByTestId("reason-fix-buffers_differ")).toBeVisible();
  await expect(page.getByTestId("device-0-buffer")).toBeVisible();
  await expect(page.getByTestId("live-gap-Studio+")).toHaveText("64 samples ahead");
  // The window itself does not scroll sideways, which is the phone rule the other pages keep.
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBeLessThanOrEqual(1);
  const main = page.locator("ga-app main");
  expect(await main.evaluate((el) => el.scrollWidth - el.clientWidth), `what runs over: ${(await overflowing(page)).join("; ")}`).toBeLessThanOrEqual(1);
});

/** Every element wider than the box it sits in, through the shadow roots, so a failure names it. */
function overflowing(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const found: string[] = [];
    const visit = (root: Document | ShadowRoot, path: string) => {
      for (const element of root.querySelectorAll("*")) {
        if (element.clientWidth > 0 && element.scrollWidth > element.clientWidth + 1) found.push(`${path}>${element.localName}.${String(element.className)} ${element.scrollWidth}/${element.clientWidth}`);
        if (element.shadowRoot !== null) visit(element.shadowRoot, `${path}>${element.localName}`);
      }
    };
    visit(document, "");
    return found.slice(0, 12);
  });
}

// ---------------------------------------------------------------------------------------------
// The tab
// ---------------------------------------------------------------------------------------------

test("the header has an Aggregate tab that opens the page", async ({ page }) => {
  await fakeAggregate(page, answer());
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await page.locator('ga-header a[data-page="aggregate"]').click();
  await expect(page.locator("ga-aggregate")).toBeVisible();
  await expect(page.locator("ga-app h1.page-title")).toHaveText("Aggregate");
});
