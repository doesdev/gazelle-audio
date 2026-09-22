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
  matched_by: "chosen",
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
  /** How many times it asked about the measurement, which it does only while one is running. */
  calibrationReads: number;
  /**
   * What `GET /aggregate/calibrate` answers next. The server has no such route in these tests, so
   * a test sets this to whatever state it wants the page to show; a list is read in turn, so a run
   * can be seen to start, get on with it and finish.
   */
  calibration: Record<string, unknown>[];
  /** When set, starting a run is refused with this message rather than started. */
  refuseCalibrate?: string;
}

/**
 * Answers `GET /aggregate` with `reading` (or the next of several, so a fix can change what comes
 * back) and captures every POST the page makes to an aggregate or command route without letting it
 * reach the server.
 */
async function fakeAggregate(page: Page, ...readings: Record<string, unknown>[]): Promise<Captured> {
  const captured: Captured = { posts: [], commands: [], reads: 0, calibrationReads: 0, calibration: [{ state: "idle" }] };
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
    const calibration = () => captured.calibration[Math.min(captured.calibrationReads, captured.calibration.length - 1)];
    if (request.method() === "POST") {
      captured.posts.push({ route: path, body: request.postDataJSON() });
      if (path.endsWith("match-buffers")) return route.fulfill({ json: { buffer_size: (request.postDataJSON() as { buffer_size: number }).buffer_size, changed: 2, refused: 0, devices: [] } });
      if (path.endsWith("calibrate/stop")) {
        const wasRunning = captured.calibration[Math.min(captured.calibrationReads, captured.calibration.length - 1)]?.["state"] === "running";
        captured.calibration = [{ state: "idle" }];
        captured.calibrationReads = 0;
        return route.fulfill({ json: { stopped: wasRunning } });
      }
      // A run the server will not start refuses, in the codebase's usual refusal shape.
      if (path.endsWith("calibrate")) {
        const refusal = captured.refuseCalibrate;
        if (refusal !== undefined) return route.fulfill({ status: 409, json: { error: { code: "asio_in_use", message: refusal } } });
        return route.fulfill({ json: { started: true } });
      }
      return route.fulfill({ json: { dll: DLL, command: "regsvr32", run: { started: true, exit_code: 0, message: "The driver was registered." } } });
    }
    if (path.endsWith("aggregate/calibrate")) {
      const state = calibration();
      captured.calibrationReads += 1;
      return route.fulfill({ json: state });
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
// Which Gazelle device an entry is
// ---------------------------------------------------------------------------------------------

test("an interface Gazelle worked out for itself is live without anybody choosing, and choosing pins it", async ({ page }) => {
  // Nothing in the workspace says which device this is: the server worked it out from the answer.
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro" }] } });
  await fakeAggregate(page, answer({ devices: [deviceReport("Quadro", { matched_by: "worked_out", is_master: true })] }));
  await open(page);

  await expect(page.getByTestId("device-0-device-id")).toHaveValue("loopback-0");
  await expect(page.getByTestId("device-0-worked-out")).toBeVisible();
  await expect(page.getByTestId("device-0-match-note")).toBeHidden();
  // The controls that need a device are live, because the server resolved one.
  await expect(page.getByTestId("device-0-buffer")).toBeEnabled();
  await expect(page.getByTestId("device-0-buffer")).toHaveValue("256");
  await expect(page.getByTestId("device-0-safe")).toBeEnabled();
  // And nothing was written: it is worked out each time rather than pinned.
  const devices = async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()).aggregate?.devices ?? []) as { device_id?: string }[];
  expect((await devices())[0]?.device_id).toBeUndefined();

  // Choosing one pins it in the workspace, which is the whole difference.
  await page.getByTestId("device-0-device-id").selectOption("loopback-1");
  await expect.poll(async () => (await devices())[0]?.device_id).toBe("loopback-1");
});

test("an interface Gazelle cannot tell shows the note saying why, and leaves its driver controls dead", async ({ page }) => {
  const why = "Two interfaces of this model are connected, so Gazelle cannot tell which one this is. Choose it here.";
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro" }] } });
  await fakeAggregate(
    page,
    answer({
      devices: [deviceReport("Quadro", { device_id: undefined, matched_by: "none", match_note: why, driver: {} })],
      reasons: [{ code: "device_not_matched", severity: "warning", message: `Gazelle cannot tell which of its devices Quadro is. ${why}`, device: "Quadro" }],
    }),
  );
  await open(page);

  await expect(page.getByTestId("device-0-match-note")).toHaveText(why);
  await expect(page.getByTestId("device-0-worked-out")).toBeHidden();
  await expect(page.getByTestId("device-0-device-id")).toHaveValue("");
  await expect(page.getByTestId("device-0-buffer")).toBeDisabled();
  // The reason is worth knowing rather than blocking, and there is nothing to press.
  await expect(page.getByTestId("reason-severity-device_not_matched")).toHaveText("WORTH KNOWING");
  await expect(page.getByTestId("reason-fix-device_not_matched")).toHaveCount(0);
  await expect(page.getByTestId("aggregate-verdict")).toHaveText("Ready");
});

// ---------------------------------------------------------------------------------------------
// The channels: which of them a DAW sees, and what it calls them
// ---------------------------------------------------------------------------------------------

const withChannels = (parts: Record<string, unknown> = {}) =>
  deviceReport("Quadro", { channels: { inputs: ["Mic 1", "Mic 2", "Mic 3", "Mic 4"], outputs: ["Main L", "Main R"], source: "gazelle" }, ...parts });

/** The one configured interface, as the server has it now. */
const configured = async (): Promise<Record<string, unknown>> => (((await (await fetch(`${server.url}/api/v1/workspace`)).json()).aggregate?.devices ?? [])[0] ?? {}) as Record<string, unknown>;

test("the channels are listed one by one, with Gazelle's own names beside them as a suggestion", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0" }] } });
  await fakeAggregate(page, answer({ devices: [withChannels()] }));
  await open(page);

  // Closed, it is one line, and the rows are not on screen.
  await expect(page.getByTestId("device-0-channels")).toHaveText("All in, All out");
  await expect(page.getByTestId("device-0-in-0-label")).toBeHidden();

  await page.getByTestId("device-0-channels-open").click();
  await expect(page.getByTestId("device-0-in-0-auto")).toHaveText("Quadro 1");
  await expect(page.getByTestId("device-0-in-3-auto")).toHaveText("Quadro 4");
  await expect(page.getByTestId("device-0-in-4")).toHaveCount(0, { timeout: 2000 });
  await expect(page.getByTestId("device-0-out-1-auto")).toHaveText("Quadro 2");
  await expect(page.getByTestId("device-0-in-1-hint")).toHaveText("Gazelle calls it Mic 2");
  await expect(page.getByTestId("device-0-out-0-hint")).toHaveText("Gazelle calls it Main L");
  await expect(page.getByTestId("device-0-in-0-label")).toHaveAttribute("placeholder", "Quadro 1");
  await expect(page.getByTestId("device-0-channels-note")).toContainText("in brackets");
});

test("with no count to go on the channels are not guessed at, and the page says why", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro" }] } });
  await fakeAggregate(page, answer({ devices: [deviceReport("Quadro", { device_id: undefined, matched_by: "none", driver: {} })] }));
  await open(page);
  await page.getByTestId("device-0-channels-open").click();
  await expect(page.getByTestId("device-0-channels-unknown")).toContainText("not known until a DAW opens the aggregate");
  await expect(page.getByTestId("device-0-in-0")).toHaveCount(0);
});

test("not exposing a channel writes the ones that are left, and exposing it again takes the field away", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0" }] } });
  await fakeAggregate(page, answer({ devices: [withChannels()] }));
  await open(page);
  await page.getByTestId("device-0-channels-open").click();

  await expect(page.getByTestId("device-0-in-2-expose")).toHaveText("On");
  await page.getByTestId("device-0-in-2-expose").click();
  await expect.poll(async () => (await configured())["inputs"]).toEqual([0, 1, 3]);
  await expect(page.getByTestId("device-0-in-2-expose")).toHaveText("Off");
  await expect(page.getByTestId("device-0-channels")).toHaveText("3 in, All out");
  // The card was rebuilt by that edit, and the part stayed open.
  await expect(page.getByTestId("device-0-in-2-expose")).toBeVisible();

  // Everything exposed again means no field at all, which is what the driver's file takes as all.
  await page.getByTestId("device-0-in-2-expose").click();
  await expect.poll(async () => Object.hasOwn(await configured(), "inputs")).toBe(false);
  await expect(page.getByTestId("device-0-channels")).toHaveText("All in, All out");
});

test("naming a channel writes the name, and clearing it takes the entry out rather than writing nothing", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0" }] } });
  await fakeAggregate(page, answer({ devices: [withChannels()] }));
  await open(page);
  await page.getByTestId("device-0-channels-open").click();

  await page.getByTestId("device-0-in-0-label").fill("Vocal mic");
  await page.getByTestId("device-0-in-0-label").press("Enter");
  await expect.poll(async () => (await configured())["input_names"]).toEqual({ "0": "Vocal mic" });
  await expect(page.getByTestId("device-0-channels")).toHaveText("All in, All out, 1 named");
  await expect(page.getByTestId("device-0-in-0-label")).toHaveValue("Vocal mic");
  // A label is at most 31 characters, and the field will not take more.
  await expect(page.getByTestId("device-0-in-0-label")).toHaveAttribute("maxlength", "31");

  await page.getByTestId("device-0-out-1-label").fill("   ");
  await page.getByTestId("device-0-out-1-label").press("Enter");
  await expect.poll(async () => Object.hasOwn(await configured(), "output_names")).toBe(false);

  await page.getByTestId("device-0-in-0-label").fill("");
  await page.getByTestId("device-0-in-0-label").press("Enter");
  await expect.poll(async () => Object.hasOwn(await configured(), "input_names")).toBe(false);
  await expect(page.getByTestId("device-0-channels")).toHaveText("All in, All out");
});

test("a poll landing leaves a label half typed where it was, and the Channels part as it was", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0" }] } });
  const captured = await fakeAggregate(page, answer({ devices: [withChannels()] }));
  await open(page);
  await page.getByTestId("device-0-channels-open").click();
  await page.getByTestId("device-0-in-1-label").fill("Snare to");

  // A poll landing rebuilds nothing, and the half-typed name is still there afterwards.
  const before = captured.reads;
  await expect.poll(() => captured.reads, { timeout: 15_000 }).toBeGreaterThan(before + 1);
  await expect(page.getByTestId("device-0-in-1-label")).toHaveValue("Snare to");
  await expect(page.getByTestId("device-0-in-1-label")).toBeFocused();

  await expect(page.getByTestId("device-0-channels-part")).toHaveAttribute("open", "");
  expect(await configured(), "and nothing half typed was written").not.toHaveProperty("input_names");

  // Leaving the field commits it, as every field on this page does. The card is rebuilt around
  // the name, and the Channels part is still open with the name where it was typed.
  await page.getByTestId("device-0-in-1-label").press("Enter");
  await expect.poll(async () => (await configured())["input_names"]).toEqual({ "1": "Snare to" });
  await expect(page.getByTestId("device-0-in-1-label")).toHaveValue("Snare to");
  await expect(page.getByTestId("device-0-channels-part")).toHaveAttribute("open", "");
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
  await expect(page.getByTestId("aggregate-event").last()).toContainText("Stalled");
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
// Lining the interfaces up
// ---------------------------------------------------------------------------------------------

/** Two interfaces with channels the page can list: four each way on the Quadro, two on the Studio+. */
const withBoth = () => [
  deviceReport("Quadro", { is_master: true, channels: { inputs: ["Mic 1", "Mic 2", "Mic 3", "Mic 4"], outputs: ["Main L", "Main R", "Cue L", "Cue R"], source: "gazelle" } }),
  deviceReport("Studio+", { device_id: "loopback-1", channels: { inputs: ["Line 1", "Line 2"], outputs: ["Out 1", "Out 2"], source: "gazelle" } }),
];

const bothConfigured = () =>
  putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0" }, { key: "Studio+", name: "Studio+", device_id: "loopback-1" }], callback_master: "Quadro" } });

/** A finished run: the Studio+ records 28 samples late, and that is the trim it implies. */
const done = (parts: Record<string, unknown> = {}, readings: Record<string, unknown>[] = []) => ({
  state: "done",
  outcome: {
    direction: "inputs",
    rate: 96000,
    buffer_size: 512,
    reference: "Quadro",
    readings:
      readings.length > 0
        ? readings
        : [
            { device: "Quadro", is_reference: true, lag_samples: 0, spread_samples: 0, clicks_found: 8, clicks_expected: 8, note: "Quadro is what the others were measured against." },
            { device: "Studio+", is_reference: false, lag_samples: 27.8, spread_samples: 0.3, clicks_found: 8, clicks_expected: 8, note: "Studio+ recorded 27.8 samples after the Quadro." },
          ],
    trims: [
      { device: "Quadro", direction: "inputs", field: "input_trim", was: 0, measured: 0, now: 0, is_reference: true, not_applied: "The reference has nothing to correct against itself." },
      { device: "Studio+", direction: "inputs", field: "input_trim", was: 0, measured: 28, now: 28, is_reference: false },
    ],
    warnings: [],
    ...parts,
  },
});

test("the cabling is named channel by channel, and follows the pickers", async ({ page }) => {
  await bothConfigured();
  await fakeAggregate(page, answer({ devices: withBoth() }));
  await open(page);

  // The input pass by default: both clicks leave the Quadro, and each interface records its own.
  await expect(page.getByTestId("calibrate-direction")).toHaveValue("inputs");
  await expect(page.getByTestId("calibrate-reference")).toHaveValue("Quadro");
  await expect(page.getByTestId("calibrate-cable-0")).toContainText("Quadro 1 into Quadro 1");
  await expect(page.getByTestId("calibrate-cable-1")).toContainText("Quadro 2 into Studio+ 1");
  await expect(page.getByTestId("calibrate-problem")).toBeHidden();

  // Choosing another output moves the cable that goes with it, and nothing else.
  await page.getByTestId("calibrate-plays-1").selectOption({ label: "Quadro 4" });
  await expect(page.getByTestId("calibrate-cable-1")).toContainText("Quadro 4 into Studio+ 1");
  await expect(page.getByTestId("calibrate-cable-0")).toContainText("Quadro 1 into Quadro 1");

  // And the other pass is the same thing the other way round: one output on each interface.
  await page.getByTestId("calibrate-direction").selectOption("outputs");
  await expect(page.getByTestId("calibrate-cable-0")).toContainText("Quadro 1 into Quadro 1");
  await expect(page.getByTestId("calibrate-cable-1")).toContainText("Studio+ 1 into Quadro 2");
});

test("a channel with a name of its own is named in the cabling with its own number after it", async ({ page }) => {
  await putWorkspace(server, {
    aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0", output_names: { "0": "Monitor L" } }, { key: "Studio+", name: "Studio+", device_id: "loopback-1" }] },
  });
  await fakeAggregate(page, answer({ devices: withBoth() }));
  await open(page);
  await expect(page.getByTestId("calibrate-cable-0")).toContainText("Monitor L (Quadro 1) into Quadro 1");
});

test("measuring asks twice, then sends exactly the channels the pickers name", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  await open(page);
  await expect(page.getByTestId("calibrate-warning")).toContainText("plays a click out of a real output");
  await expect(page.getByTestId("calibrate-warning")).toContainText("close any DAW");

  const measure = page.getByTestId("calibrate-measure");
  await measure.click();
  await expect(measure).toHaveText("Confirm");
  await page.waitForTimeout(300);
  expect(captured.posts, "one click plays nothing").toEqual([]);

  captured.calibration = [{ state: "running", step: "Playing the clicks", progress: 0.42 }, done()];
  await measure.click();
  await expect.poll(() => captured.posts).toEqual([
    { route: "aggregate/calibrate", body: { direction: "inputs", outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }], inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }], clicks: 8, level_dbfs: -20 } },
  ]);
});

/**
 * **The Check that was refused on the owner's rig.** Gazelle's own list has fourteen inputs for the
 * Quadro while its driver has sixteen, so a page that counted the aggregate's channels itself sent
 * the Studio+'s first input as 14, which is the Quadro's fifteenth. A Check now names every cable end
 * by interface and that interface's own channel, so no count on this page can move a cable.
 */
test("a check sends each cable end as an interface and that interface's own channel", async ({ page }) => {
  await putWorkspace(server, {
    aggregate: {
      devices: [
        { key: "Zen Quadro Synergy Core", name: "Zen Quadro Synergy Core", device_id: "loopback-0" },
        { key: "ZenStudioTB ASIO Driver", name: "ZenStudioTB ASIO Driver", device_id: "loopback-1" },
      ],
      callback_master: "Zen Quadro Synergy Core",
    },
  });
  const fourteen = Array.from({ length: 14 }, (_, at) => `Mic ${at + 1}`);
  const captured = await fakeAggregate(
    page,
    answer({
      devices: [
        deviceReport("Zen Quadro Synergy Core", { is_master: true, channels: { inputs: fourteen, outputs: fourteen, source: "gazelle" } }),
        deviceReport("ZenStudioTB ASIO Driver", { device_id: "loopback-1", channels: { inputs: fourteen.slice(0, 8), outputs: fourteen.slice(0, 8), source: "gazelle" } }),
      ],
    }),
  );
  await open(page);
  await expect(page.getByTestId("calibrate-cable-1")).toContainText("Zen Quadro Synergy Core 2 into ZenStudioTB ASIO Driver 1");

  // The Studio+'s second input, which is its own channel 1 however many the Quadro is counted as.
  await page.getByTestId("calibrate-records-1").selectOption({ label: "ZenStudioTB ASIO Driver 2" });
  await expect(page.getByTestId("calibrate-cable-1")).toContainText("Zen Quadro Synergy Core 2 into ZenStudioTB ASIO Driver 2");

  captured.calibration = [done({ checking: true })];
  const check = page.getByTestId("calibrate-check");
  await check.click();
  await check.click();
  await expect.poll(() => captured.posts).toEqual([
    {
      route: "aggregate/calibrate",
      body: {
        direction: "inputs",
        outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }],
        inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 1 }],
        clicks: 8,
        level_dbfs: -20,
        check: true,
      },
    },
  ]);
});

test("a run shows its step and how far along it is, and can be stopped", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.calibration = [{ state: "running", step: "Playing the clicks", progress: 0.42 }];
  await open(page);
  await expect(page.getByTestId("calibrate-step")).toHaveText("Playing the clicks (42%)");
  await expect(page.getByTestId("calibrate-measure")).toBeHidden();
  // While it runs the pickers are left alone, so nothing can be changed under it.
  await expect(page.getByTestId("calibrate-direction")).toBeDisabled();

  // It is asked about again while it goes, which is the only time it is asked about at all.
  await expect.poll(() => captured.calibrationReads, { timeout: 5000 }).toBeGreaterThan(2);

  await page.getByTestId("calibrate-stop").click();
  await expect.poll(() => captured.posts.map((post) => post.route)).toEqual(["aggregate/calibrate/stop"]);
  await expect(page.getByTestId("calibrate-measure")).toBeVisible();
  const settled = captured.calibrationReads;
  await page.waitForTimeout(2000);
  expect(captured.calibrationReads, "nothing running is nothing to ask about").toBeLessThanOrEqual(settled + 1);
});

test("a finished run says how far out each interface is, how steady it was, and what it found", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.calibration = [done()];
  await open(page);

  await expect(page.getByTestId("calibrate-summary")).toContainText("96 kHz, 512 samples, against Quadro");
  await expect(page.getByTestId("calibrate-lag-Quadro")).toContainText("measured against");
  await expect(page.getByTestId("calibrate-lag-Studio+")).toHaveText("27.8 samples late");
  await expect(page.getByTestId("calibrate-lag-Studio+")).toHaveAttribute("data-tone", "off");
  await expect(page.getByTestId("calibrate-spread-Studio+")).toHaveText("Clicks agreed to 0.3 samples");
  await expect(page.getByTestId("calibrate-clicks-Studio+")).toHaveText("8 of 8 clicks found");
  await expect(page.getByTestId("calibrate-note-Studio+")).toContainText("27.8 samples after the Quadro");
  // The reference's own trim is shown with the reason it is not one the button writes.
  await expect(page.getByTestId("calibrate-trim-0-not-applied")).toContainText("nothing to correct against itself");
  await expect(page.getByTestId("calibrate-drift")).toBeHidden();

  // The trim it implies: what it is now, what was measured, and what it would become.
  await expect(page.getByTestId("calibrate-trim-1")).toContainText("Studio+, input trim");
  await expect(page.getByTestId("calibrate-trim-1-was")).toHaveText("0");
  await expect(page.getByTestId("calibrate-trim-1-measured")).toHaveText("28");
  await expect(page.getByTestId("calibrate-trim-1-now")).toHaveText("28");
});

test("the measured trims are written into the setup by one button, and show up in the card", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.calibration = [done()];
  await open(page);

  await expect(page.getByTestId("device-1-in-trim")).toHaveValue("0");
  await page.getByTestId("calibrate-apply").click();
  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()).aggregate?.devices ?? [])[1]?.input_trim).toBe(28);
  await expect(page.getByTestId("device-1-in-trim")).toHaveValue("28");
  await expect(page.getByTestId("calibrate-applied")).toContainText("Studio+ in 28");
});

test("a run the server will not start says why, in the server's own words", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.refuseCalibrate = "A DAW has the audio drivers open. Close it and measure again.";
  await open(page);
  await page.getByTestId("calibrate-measure").click();
  await page.getByTestId("calibrate-measure").click();
  await expect(page.getByTestId("calibrate-refusal")).toContainText("A DAW has the audio drivers open");
});

test("a run that failed shows the refusal the server gave, naming the cable to check", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.calibration = [{ state: "failed", refusal: "Nothing arrived on Studio+ 1. Check the cable from Quadro 2 into it." }];
  await open(page);
  await expect(page.getByTestId("calibrate-refusal")).toContainText("Check the cable from Quadro 2 into it");
  await expect(page.getByTestId("calibrate-trims")).toBeEmpty();
});

test("a drift finding is said as the serious one it is, and no trim is offered as the answer", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.calibration = [
    done({ trims: [] }, [
      { device: "Quadro", is_reference: true, lag_samples: 0, spread_samples: 0, clicks_found: 8 },
      { device: "Studio+", is_reference: false, lag_samples: 31, spread_samples: 12.4, clicks_found: 8, drift: { samples_per_second: 0.6, ppm: 6.25, real: true } },
    ]),
  ];
  await open(page);
  await expect(page.getByTestId("calibrate-drift")).toContainText("not sharing one clock");
  await expect(page.getByTestId("calibrate-drift-Studio+")).toContainText("no trim can put that right");
  await expect(page.getByTestId("calibrate-lag-Studio+")).toHaveAttribute("data-tone", "drift");
  await expect(page.getByTestId("calibrate-apply")).toBeHidden();
});

test("with one interface there is nothing to line up, and the page says so instead of measuring", async ({ page }) => {
  await putWorkspace(server, { aggregate: { devices: [{ key: "Quadro", name: "Quadro", device_id: "loopback-0" }] } });
  await fakeAggregate(page, answer({ devices: [withBoth()[0] as Record<string, unknown>] }));
  await open(page);
  await expect(page.getByTestId("calibrate-problem")).toContainText("at least two interfaces");
  await expect(page.getByTestId("calibrate-measure")).toBeDisabled();
});

test("a poll landing leaves a picker where it was put", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  await open(page);
  await page.getByTestId("calibrate-plays-1").selectOption({ label: "Quadro 4" });
  const before = captured.reads;
  await expect.poll(() => captured.reads, { timeout: 15_000 }).toBeGreaterThan(before + 1);
  await expect(page.getByTestId("calibrate-plays-1")).toHaveValue("3");
  await expect(page.getByTestId("calibrate-cable-1")).toContainText("Quadro 4 into Studio+ 1");
});

test("a server too old to measure says so rather than failing", async ({ page }) => {
  await bothConfigured();
  await fakeAggregate(page, answer({ devices: withBoth() }));
  // Added after the fake, so it is the one that answers: the later route wins.
  await page.route("**/api/v1/aggregate/calibrate**", (route) => route.fulfill({ status: 404, body: "" }));
  await open(page);
  await expect(page.getByTestId("calibrate-unavailable")).toBeVisible();
  await expect(page.getByTestId("calibrate-measure")).toBeDisabled();
});

// ---------------------------------------------------------------------------------------------
// The phase: its setup on each follower's card, the check, and what a run carries
// ---------------------------------------------------------------------------------------------

/** Both interfaces, with the Studio+'s phase path set and whatever else a test gives it. */
const phaseConfigured = (studio: Record<string, unknown> = {}) =>
  putWorkspace(server, {
    aggregate: {
      devices: [
        { key: "Quadro", name: "Quadro", device_id: "loopback-0" },
        { key: "Studio+", name: "Studio+", device_id: "loopback-1", phase: { master_output: 3, input: 1 }, ...studio },
      ],
      callback_master: "Quadro",
    },
  });

/** The Studio+ as the server has it now. */
const studio = async (): Promise<Record<string, unknown>> => (((await (await fetch(`${server.url}/api/v1/workspace`)).json()).aggregate?.devices ?? [])[1] ?? {}) as Record<string, unknown>;

test("a follower's phase is set up by two pickers, written only once both are chosen, and cleared", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  await open(page);

  // Not offered on the callback master's card: the others are measured against it.
  await expect(page.getByTestId("device-0-phase-part")).toBeHidden();
  await expect(page.getByTestId("device-0-phase")).toHaveText("The others are measured against it");

  await expect(page.getByTestId("device-1-phase-summary")).toHaveText("Not set up");
  await page.getByTestId("device-1-phase-open").click();
  // The master's own outputs and this interface's own inputs, counted from one like the rest of the page.
  await expect(page.getByTestId("device-1-phase-leaves").locator("option")).toHaveText(["Choose a channel", "Quadro 1 (Main L)", "Quadro 2 (Main R)", "Quadro 3 (Cue L)", "Quadro 4 (Cue R)"]);
  await expect(page.getByTestId("device-1-phase-arrives").locator("option")).toHaveText(["Choose a channel", "Studio+ 1 (Line 1)", "Studio+ 2 (Line 2)"]);
  // The routing it needs, naming both ends.
  await expect(page.getByTestId("device-1-phase-routing")).toContainText("on Quadro, route the playback channel chosen under Leaves the callback master on to its S/PDIF output");
  await expect(page.getByTestId("device-1-phase-routing")).toContainText("on Studio+, route its S/PDIF input");

  // One picker alone writes nothing, because the driver refuses half a path, and a poll leaves it be.
  await page.getByTestId("device-1-phase-leaves").selectOption({ label: "Quadro 4 (Cue R)" });
  const before = captured.reads;
  await expect.poll(() => captured.reads, { timeout: 15_000 }).toBeGreaterThan(before);
  await expect(page.getByTestId("device-1-phase-leaves")).toHaveValue("3");
  expect(await studio(), "half a path is not written").not.toHaveProperty("phase");

  // The second writes both, in the devices' own numbering from zero.
  await page.getByTestId("device-1-phase-arrives").selectOption({ label: "Studio+ 2 (Line 2)" });
  await expect.poll(async () => (await studio())["phase"]).toEqual({ master_output: 3, input: 1 });
  await expect(page.getByTestId("device-1-phase-summary")).toHaveText("Set up, no reference yet");
  await expect(page.getByTestId("device-1-phase-note")).toContainText("One measurement under Line the interfaces up gives it one");
  // The card was rebuilt by that edit, and the part is still open.
  await expect(page.getByTestId("device-1-phase-part")).toHaveAttribute("open", "");

  // Clearing asks twice, and takes the whole setting out.
  await page.getByTestId("device-1-phase-clear").click();
  await expect(page.getByTestId("device-1-phase-clear")).toHaveText("Confirm");
  await page.getByTestId("device-1-phase-clear").click();
  await expect.poll(async () => Object.hasOwn(await studio(), "phase")).toBe(false);
  await expect(page.getByTestId("device-1-phase-summary")).toHaveText("Not set up");
});

test("the phase not measured reason says where to set it up, and its button opens that card's setup", async ({ page }) => {
  await bothConfigured();
  await fakeAggregate(
    page,
    answer({
      devices: withBoth(),
      reasons: [{ code: "phase_not_measured", severity: "warning", message: "Studio+ has a S/PDIF cable from Quadro and has not been set up for phase measurement.", device: "Studio+", device_id: "loopback-1" }],
    }),
  );
  await open(page);
  await expect(page.getByTestId("reason-severity-phase_not_measured")).toHaveText("WORTH KNOWING");
  await expect(page.getByTestId("reason-hint-phase_not_measured")).toContainText("under Phase on Studio+'s card");
  await expect(page.getByTestId("device-1-phase-part")).not.toHaveAttribute("open", "");
  await page.getByTestId("reason-goto-phase_not_measured").click();
  await expect(page.getByTestId("device-1-phase-part")).toHaveAttribute("open", "");
  await expect(page.getByTestId("device-1-phase-leaves")).toBeFocused();
});

test("checking asks twice, sends a check, and reads as a verdict with no trims to write", async ({ page }) => {
  await bothConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  await open(page);
  await expect(page.getByTestId("calibrate-check-note")).toContainText("Check lines the session up exactly as a DAW's is");
  await expect(page.getByTestId("calibrate-routing-note")).toContainText("its S/PDIF output");

  const check = page.getByTestId("calibrate-check");
  await check.click();
  await expect(check).toHaveText("Confirm");
  await page.waitForTimeout(300);
  expect(captured.posts, "one click plays nothing").toEqual([]);

  captured.calibration = [
    done(
      {
        checking: true,
        trims: [{ device: "Studio+", direction: "inputs", field: "input_trim", was: 28, measured: 0, now: 28, is_reference: false }],
        clean: true,
        blocks_lost: 0,
        phases: [{ device: "Studio+", state: "applied", measured_samples: -148, applied_samples: -64, note: "Studio+ was lined up to its reference." }],
      },
      [
        { device: "Quadro", is_reference: true, lag_samples: 0, spread_samples: 0, clicks_found: 8, clicks_expected: 8 },
        { device: "Studio+", is_reference: false, lag_samples: 0.02, spread_samples: 0.01, clicks_found: 8, clicks_expected: 8 },
      ],
    ),
  ];
  await check.click();
  await expect.poll(() => captured.posts).toEqual([
    { route: "aggregate/calibrate", body: { direction: "inputs", outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }], inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }], clicks: 8, level_dbfs: -20, check: true } },
  ]);
  await expect(page.getByTestId("calibrate-summary")).toContainText("Checked what the interfaces record");
  await expect(page.getByTestId("calibrate-verdict-text-Studio+")).toHaveText("Lined up: a recording would land 0.02 samples late against Quadro.");
  await expect(page.getByTestId("calibrate-verdict-text-Studio+")).toHaveAttribute("data-tone", "good");
  await expect(page.getByTestId("calibrate-verdict-Quadro")).toHaveCount(0);
  await expect(page.getByTestId("calibrate-phase-state-Studio+")).toHaveText("Lined up to its reference");
  await expect(page.getByTestId("calibrate-clean")).toHaveText("Clean: no interface lost a block while it ran.");
  // A verdict, not an offer: no trims, and nothing to write.
  await expect(page.getByTestId("calibrate-trims")).toBeEmpty();
  await expect(page.getByTestId("calibrate-apply")).toBeHidden();
  await expect(page.getByTestId("calibrate-lag-Studio+")).toHaveCount(0);
});

test("writing a measured trim writes the phase reference measured beside it", async ({ page }) => {
  await phaseConfigured();
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.calibration = [
    done({
      clean: true,
      blocks_lost: 0,
      trims: [
        { device: "Quadro", direction: "inputs", field: "input_trim", was: 0, measured: 0, now: 0, is_reference: true, not_applied: "The reference has nothing to correct against itself." },
        { device: "Studio+", direction: "inputs", field: "input_trim", was: 0, measured: 28, now: 28, is_reference: false, phase_reference: { was: null, now: -84 } },
      ],
      phases: [
        { device: "Quadro", state: "not_configured", measured_samples: 0, applied_samples: 0, note: "" },
        { device: "Studio+", state: "measured_only", measured_samples: -84, applied_samples: 0, note: "Studio+ was measured at -84 samples." },
      ],
    }),
  ];
  await open(page);
  await expect(page.getByTestId("device-1-phase-summary")).toHaveText("Set up, no reference yet");
  await expect(page.getByTestId("calibrate-phase-state-Studio+")).toContainText("not applied, on purpose");
  await expect(page.getByTestId("calibrate-phase-figures-Studio+")).toHaveText("Measured -84 samples, applied 0 samples");
  await expect(page.getByTestId("calibrate-trim-1-reference")).toHaveText("Phase reference: none yet, becomes -84 samples");

  await page.getByTestId("calibrate-apply").click();
  await expect.poll(async () => [(await studio())["input_trim"], (await studio())["phase"]]).toEqual([28, { master_output: 3, input: 1, reference: -84 }]);
  await expect(page.getByTestId("calibrate-applied")).toHaveText("1 trim written: Studio+ in 28 (phase reference -84).");
  await expect(page.getByTestId("device-1-phase-summary")).toHaveText("Set up, reference -84 samples");
});

test("a first run whose trim comes out unchanged still writes its new reference", async ({ page }) => {
  await phaseConfigured({ input_trim: 28 });
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.calibration = [
    done({
      trims: [{ device: "Studio+", direction: "inputs", field: "input_trim", was: 28, measured: 28, now: 28, is_reference: false, phase_reference: { was: null, now: -148 } }],
    }),
  ];
  await open(page);
  // The trim alone would be nothing to write; the reference beside it is new, so the button is there.
  await expect(page.getByTestId("calibrate-trim-0-was")).toHaveText("28");
  await expect(page.getByTestId("calibrate-trim-0-now")).toHaveText("28");
  await expect(page.getByTestId("calibrate-apply")).toBeVisible();
  await page.getByTestId("calibrate-apply").click();
  await expect.poll(async () => (await studio())["phase"]).toEqual({ master_output: 3, input: 1, reference: -148 });
  expect((await studio())["input_trim"]).toBe(28);
});

test("a run that was not clean, or whose phase was refused, reads as such", async ({ page }) => {
  await phaseConfigured({ input_trim: 28, phase: { master_output: 3, input: 1, reference: -84 } });
  const captured = await fakeAggregate(page, answer({ devices: withBoth() }));
  captured.calibration = [
    done(
      {
        clean: false,
        blocks_lost: 4,
        trims: [{ device: "Studio+", direction: "inputs", field: "input_trim", was: 28, measured: 31, now: 31, is_reference: false, phase_reference: { was: -84, now: null } }],
        phases: [{ device: "Studio+", state: "not_heard", measured_samples: 0, applied_samples: 0, note: "Nothing arrived on its measurement channel." }],
        witnesses: [{ channel: 1, device: "Studio+", lag_samples: 3.2, spread_samples: 0.1, clicks_found: 8, clicks_expected: 8, note: "Studio+ 2 recorded it 3.2 samples late." }],
      },
      [
        { device: "Quadro", is_reference: true, lag_samples: 0, spread_samples: 0, clicks_found: 8, clicks_expected: 8 },
        { device: "Studio+", is_reference: false, lag_samples: 31, spread_samples: 0.4, spread_limit_samples: 7, clicks_found: 8, clicks_expected: 8, blocks_dropped: 3, blocks_starved: 1 },
      ],
    ),
  ];
  await open(page);
  await expect(page.getByTestId("calibrate-clean")).toContainText("Not clean: 4 blocks lost while it ran.");
  await expect(page.getByTestId("calibrate-clean")).toHaveClass(/problem/);
  await expect(page.getByTestId("calibrate-lost-Studio+")).toContainText("dropped 3 blocks and missed 1 block");
  await expect(page.getByTestId("calibrate-spread-Studio+")).toHaveText("Clicks agreed to 0.4 samples, within the 7 allowed");
  await expect(page.getByTestId("calibrate-phase-refused")).toContainText("The phase was not measured on Studio+");
  await expect(page.getByTestId("calibrate-phase-state-Studio+")).toHaveText("Refused: nothing heard on the cable");
  await expect(page.getByTestId("calibrate-phase-state-Studio+")).toHaveAttribute("data-tone", "off");
  await expect(page.getByTestId("calibrate-trim-0-reference")).toContainText("-84 samples is taken out");
  await expect(page.getByTestId("calibrate-witness-0")).toContainText("Studio+ 2, on Studio+");
  await expect(page.getByTestId("calibrate-witness-0-lag")).toHaveText("3.2 samples late");

  // Writing it takes the old reference out rather than leaving it beside the new trim.
  await page.getByTestId("calibrate-apply").click();
  await expect.poll(async () => [(await studio())["input_trim"], (await studio())["phase"]]).toEqual([31, { master_output: 3, input: 1 }]);
});

test("while a DAW has it open each follower's phase is read out beside its gap", async ({ page }) => {
  await phaseConfigured({ phase: { master_output: 3, input: 1, reference: -84 } });
  await fakeAggregate(
    page,
    answer({
      devices: withBoth(),
      status: streaming([
        liveDevice("Quadro", { is_master: true, phase: "not_configured", phase_measured: 0, phase_applied: 0 }),
        liveDevice("Studio+", { phase: "applied", phase_measured: -148, phase_applied: -64 }),
      ]),
    }),
  );
  await open(page);
  await expect(page.getByTestId("live-phase-Studio+")).toHaveText("Phase: Lined up to its reference. Measured -148 samples, applied -64 samples");
  await expect(page.getByTestId("live-phase-Studio+")).toHaveAttribute("data-tone", "good");
  await expect(page.getByTestId("live-phase-Quadro")).toHaveCount(0);
  await expect(page.getByTestId("device-1-phase")).toHaveText("Lined up to its reference. Measured -148 samples, applied -64 samples");
  await expect(page.getByTestId("device-1-gap")).toHaveText("In step");
});

test("a refused phase reads as refused, live", async ({ page }) => {
  await phaseConfigured();
  await fakeAggregate(page, answer({ devices: withBoth(), status: streaming([liveDevice("Quadro", { is_master: true }), liveDevice("Studio+", { phase: "not_heard", phase_measured: 0, phase_applied: 0 })]) }));
  await open(page);
  await expect(page.getByTestId("live-phase-Studio+")).toHaveText("Phase: Refused: nothing heard on the cable");
  await expect(page.getByTestId("live-phase-Studio+")).toHaveAttribute("data-tone", "off");
});

test("the log says phase and lost blocks in words, and marks a Gazelle measurement's lines as Gazelle's", async ({ page }) => {
  await fakeAggregate(
    page,
    answer({
      events: [
        { at: "2026-09-21 21:14:07", kind: "session-started", message: "40 in, 40 out at 96000 Hz" },
        { at: "2026-09-21 21:14:09", kind: "phase", message: "Gazelle's own measurement: Studio+ was measured at -84 samples from where its driver's figures put it." },
        { at: "2026-09-21 21:47:02", kind: "glitched", message: "Studio+ dropped a block, the first this session has lost" },
        { at: "2026-09-21 22:02:11", kind: "session-ended", message: "ran for 47 minutes 12 seconds. Quadro lost nothing; Studio+ dropped 3 blocks" },
      ],
    }),
  );
  await open(page);
  await expect(page.getByTestId("aggregate-event-kind")).toHaveText(["Session started", "Phase measured", "Lost a block", "Session ended"]);
  await expect(page.getByTestId("aggregate-event-gazelle")).toHaveCount(1);
  await expect(page.getByTestId("aggregate-event").nth(1)).toHaveAttribute("data-gazelle", "");
  await expect(page.getByTestId("aggregate-event-message").nth(1)).toHaveText("Studio+ was measured at -84 samples from where its driver's figures put it.");
  await expect(page.getByTestId("aggregate-event-kind").nth(2)).toHaveAttribute("data-problem", "");
  await expect(page.getByTestId("aggregate-event-message").nth(3)).toContainText("Studio+ dropped 3 blocks");
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
      devices: [withChannels({ is_master: true }), deviceReport("Studio+")],
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
  // Every channel row is on screen too, which is the widest thing this page has.
  await page.getByTestId("device-0-channels-open").click();
  await expect(page.getByTestId("device-0-in-0-label")).toBeVisible();
  // And the measurement's pickers and its cabling, which are the other things that sit side by side.
  await expect(page.getByTestId("calibrate-plays-0")).toBeVisible();
  await expect(page.getByTestId("calibrate-cable-0")).toBeVisible();
  await expect(page.getByTestId("calibrate-check")).toBeVisible();
  // And a follower's phase setup, with its two pickers and the routing it needs.
  await page.getByTestId("device-1-phase-open").click();
  await expect(page.getByTestId("device-1-phase-leaves")).toBeVisible();
  await expect(page.getByTestId("device-1-phase-arrives")).toBeVisible();
  await expect(page.getByTestId("device-1-phase-routing")).toBeVisible();
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
