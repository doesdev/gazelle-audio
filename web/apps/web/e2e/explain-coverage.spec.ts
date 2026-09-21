// The explain mode's coverage guard: every page, the shell around it, the sidebar, the dock, the
// Control Room, notices and the Effects page, on both models, walked through every shadow root. It
// fails when an interactive element carries no `data-explain` key, or a key the catalogue has no entry
// for, so a new control cannot ship unexplained. Readouts, badges, meters and section headings are held
// to the same rule. Hidden elements count too (a link bar not yet opened, a drawer's close button, a
// popover not yet shown): they are part of the page, and will be seen.
//
// The server here is the loopback without dry run, so reads answer and the pages build what a device
// would give them: loaded effect chains, an effect's editor, a snapshot and its comparison.

import { expect, test, type Page } from "@playwright/test";

import { CATALOGUE } from "../src/elements/explain-catalogue.ts";
import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "200"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

/** Anything a person can press, type in, pick, drag or tab to. */
const INTERACTIVE = 'button, input, select, textarea, summary, a[href], [role="slider"], [role="switch"], [role="button"], [tabindex]:not([tabindex="-1"])';
/** What the explain mode also promises to explain: readouts, badges, meters and headings. */
const EXPLAINED = 'h2, .readout, .badge, [role="img"], .meter, .effect-meter, ga-output-meters .output, .doubled, .lock, .hpf';

interface Found {
  what: string;
  key: string | null;
}

/** Every element matching either list, in the document and every open shadow root, with its key. */
function walk(page: Page): Promise<Found[]> {
  return page.evaluate(
    ([interactive, explained]) => {
      const found: { what: string; key: string | null }[] = [];
      const describe = (element: Element, hosts: string[]) => {
        const bits = [element.localName];
        for (const attribute of ["data-testid", "aria-label", "class", "title"]) {
          const value = element.getAttribute(attribute);
          if (value) bits.push(`${attribute}="${value.slice(0, 60)}"`);
        }
        const text = (element.textContent ?? "").trim().slice(0, 30);
        return `${hosts.join(" > ")} > ${bits.join(" ")}${text ? ` "${text}"` : ""}`;
      };
      const visit = (root: Document | ShadowRoot, hosts: string[]) => {
        for (const element of root.querySelectorAll("*")) {
          // The explain mode's own panel explains, and is not explained.
          if (element.closest('[data-testid="explain-panel"]') === null && (element.matches(interactive) || element.matches(explained))) {
            found.push({ what: describe(element, hosts), key: element.getAttribute("data-explain") });
          }
          if (element.shadowRoot !== null) visit(element.shadowRoot, [...hosts, element.localName]);
        }
      };
      visit(document, []);
      return found;
    },
    [INTERACTIVE, EXPLAINED] as const,
  );
}

const problems: string[] = [];
let checked = 0;
const keysSeen = new Set<string>();

async function check(page: Page, where: string): Promise<void> {
  const found = await walk(page);
  expect(found.length, `${where}: something to check`).toBeGreaterThan(5);
  for (const { what, key } of found) {
    checked++;
    if (key === null) problems.push(`${where}: no key on ${what}`);
    else if (CATALOGUE[key] === undefined) problems.push(`${where}: no entry for "${key}" on ${what}`);
    else keysSeen.add(key);
  }
}

const QUADRO = "loopback-0";
const STUDIO = "loopback-1";

const channel = (id: string, name: string, slot: number, source: number | undefined, main?: number, sends: number[] = []) => ({
  id,
  name,
  slot,
  sends,
  ...(source === undefined ? {} : { source: { group: 0, channel: source } }),
  ...(main === undefined ? {} : { main_mix: main }),
});

async function workspace(): Promise<void> {
  await putWorkspace(server, {
    groups: [{ id: "wg", name: "Band", collapsed: false, children: [] }],
    mixers: {
      // Two channels on one input in one mix: the doubled badge. A group with a band.
      [QUADRO]: {
        mixes: [{ name: "Monitors" }, { name: "Cue" }],
        groups: [{ id: "g", name: "Drums", collapsed: false, color: "#b5473a" }],
        channels: [{ ...channel("a", "Kick", 6, 0, 0, [1]), group: "g" }, { ...channel("b", "Snare", 7, 0, 0), group: "g" }, channel("c", "Vox", 8, 1, 0)],
      },
      // Nothing set up: the starting layouts.
      [STUDIO]: { mixes: [{ name: "Main" }], channels: [channel("k", "", 0, undefined)] },
    },
    links: [{ id: "l1", kind: "mixer", mode: "absolute", members: [{ device_id: QUADRO, channel: 8 }, { device_id: QUADRO, channel: 9 }] }],
    surfaces: [
      {
        id: "s",
        name: "Everything",
        mixes: {},
        strips: [
          { id: "ch", kind: "channel", device_id: QUADRO, channel: "a" },
          { id: "ms", kind: "master", device_id: QUADRO, mix: 1 },
          { id: "pre", kind: "input", device_id: STUDIO, input: { kind: "preamp", channel: 0 } },
          { id: "sp", kind: "input", device_id: STUDIO, input: { kind: "spdif", channel: 0 } },
          { id: "ad", kind: "input", device_id: QUADRO, input: { kind: "adat", channel: 0 } },
          { id: "out", kind: "output", device_id: STUDIO, output: 1 },
          { id: "port", kind: "port", device_id: QUADRO, port: "SPDIF_OUT", first: 0 },
          { id: "lab", kind: "label", text: "Drums" },
        ],
      },
    ],
    cables: [{ id: "c1", from: { device_id: STUDIO, port: "ADAT_OUT", first: 0 }, to: { device_id: QUADRO, port: "ADAT_IN", first: 0 }, channels: 8 }],
    control_room: { [QUADRO]: { outputs: [0, 1, 2, 3] } },
    // Two interfaces in the aggregate, so its page has a card for each with every control on it.
    aggregate: { devices: [{ key: "Zen Quadro Synergy Core", name: "Quadro", device_id: QUADRO, input_trim: 8 }, { key: "Zen Studio+", name: "Studio+", device_id: STUDIO }], callback_master: "Quadro", alignment: "aligned" },
  });
}

/**
 * The Aggregate page as a working aggregate fills it: a blocking reason with its fix, a warning,
 * a registration pointing at a copy that has gone, and a DAW streaming with a plan, a gap, a stall,
 * a refusal and an event log. None of it reaches a driver.
 */
const AGGREGATE_ANSWER = {
  read_at_ms: 1,
  configured: true,
  export_path: "C:\\gazelle\\aggregate.json",
  drivers: [
    { key: "Zen Quadro Synergy Core", description: "Zen Quadro Synergy Core", clsid: "{1}", dll: "q.dll", dll_present: true, configured: true, is_aggregate: false },
    { key: "Zen Studio+", description: "Zen Studio+", clsid: "{2}", dll: "s.dll", dll_present: true, configured: true, is_aggregate: false },
    { key: "Other interface", description: "Other interface", clsid: "{4}", dll: "o.dll", dll_present: true, configured: false, is_aggregate: false },
  ],
  registration: {
    registered: true,
    clsid: "{3}",
    name: "Gazelle Aggregate",
    dll: "C:\\old\\gazelle_aggregate.dll",
    dll_present: false,
    message: "Registered, pointing at a copy that has gone.",
    register_command: "regsvr32 /s gazelle_aggregate.dll",
    unregister_command: "regsvr32 /s /u gazelle_aggregate.dll",
    dll_search: { state: "found", dll: "C:\\gazelle\\gazelle_aggregate.dll" },
  },
  devices: [
    { name: "Quadro", key: "Zen Quadro Synergy Core", registered: true, entry_key: "Zen Quadro Synergy Core", device_id: QUADRO, matched_by: "chosen", channels: { inputs: ["Mic 1", "Mic 2"], outputs: ["Monitor L", "Monitor R"], source: "gazelle" }, attached: true, family: "quadro", clock: { source_index: 0, source: "Internal", locked: true, hz: 96000, rate_index: 4 }, driver: { sample_rate: 96000, buffer_size: 256, safe_mode: false, asio_clients: 0 }, is_master: true },
    { name: "Studio+", key: "Zen Studio+", registered: true, entry_key: "Zen Studio+", device_id: STUDIO, matched_by: "worked_out", channels: { inputs: ["Line 1", "Line 2"], outputs: ["Main L", "Main R"], source: "gazelle" }, attached: true, family: "studio", clock: { source_index: 0, source: "Internal", locked: false, hz: 48000, rate_index: 2 }, driver: { sample_rate: 48000, buffer_size: 128, safe_mode: true, asio_clients: 0 }, is_master: false },
  ],
  ready: false,
  reasons: [
    { code: "buffers_differ", severity: "blocking", message: "The drivers are on different buffer sizes (256 and 128).", fix: { kind: "match_buffers", method: "POST", route: "aggregate/match-buffers", body: { buffer_size: 256 }, label: "Put them all on 256 samples" } },
    { code: "controller_unknown", severity: "warning", message: "Quadro's USB host controller could not be found." },
  ],
  status: {
    state: "read",
    open: true,
    streaming: true,
    generation: 5,
    generation_in_force: 4,
    up_to_date: false,
    plan: { master: "Quadro", rate: 96000, buffer_size: 256, inputs: 40, outputs: 40, alignment: "aligned", input_latency: 611, output_latency: 733 },
    devices: [
      { name: "Quadro", driver_name: "Quadro", streaming: true, stalled: false, is_master: true, sample_gap: 0, callbacks: 1200, dropped: 0, starved: 0 },
      { name: "Studio+", driver_name: "Studio+", streaming: false, stalled: true, is_master: false, sample_gap: -64, callbacks: 900, dropped: 2, starved: 3 },
    ],
    last_refusal: "Studio+ will not run at 96000 Hz, so neither will the aggregate",
  },
  events: [{ at: "2026-09-21 09:14:02", kind: "session-started", message: "40 in, 40 out at 96000 Hz" }],
};

/**
 * A finished measurement, so the section that lines the interfaces up is walked with its readings,
 * a drift finding and the trims it implies on screen as well as its pickers.
 */
const CALIBRATION = {
  state: "done",
  outcome: {
    direction: "inputs",
    rate: 96000,
    buffer_size: 512,
    reference: "Quadro",
    readings: [
      { device: "Quadro", is_reference: true, lag_samples: 0, spread_samples: 0, clicks_found: 8, clicks_expected: 8, note: "Quadro is what the others were measured against." },
      { device: "Studio+", is_reference: false, lag_samples: 27.8, spread_samples: 0.3, clicks_found: 8, clicks_expected: 8, note: "Studio+ recorded 27.8 samples after the Quadro.", drift: { samples_per_second: 0.6, ppm: 6.25, real: true } },
    ],
    trims: [
      { device: "Quadro", direction: "inputs", field: "input_trim", was: 0, measured: 0, now: 0, is_reference: true, not_applied: "The reference has nothing to correct against itself." },
      { device: "Studio+", direction: "inputs", field: "input_trim", was: 0, measured: 28, now: 28, is_reference: false },
    ],
    warnings: ["The Studio+ was 3 dB quieter than the Quadro, which does not change the measurement."],
  },
};

test("every control, readout, badge and heading on every page carries a key the catalogue explains", async ({ page }) => {
  test.setTimeout(240_000);
  await workspace();

  const visit = async (hash: string, ready: string) => {
    await page.goto(`${server.url}/#/${hash}`);
    await expect(page.locator(ready).first()).toBeVisible();
    // Let reads land, so what they build is on the page.
    await page.waitForTimeout(600);
  };

  // The explain mode on, so its own controls are on the page too.
  await visit(`devices/${QUADRO}`, "ga-device-status");
  await page.getByRole("button", { name: "Explain mode" }).click();
  await check(page, `devices ${QUADRO}`);
  await visit(`devices/${STUDIO}`, "ga-device-status");
  await check(page, `devices ${STUDIO}`);

  // The Driver section as a driver fills it (the loopback has none): its buffer menu with the
  // Confirm beside it, the Safe Mode switch, and the Change anyway a program using ASIO brings.
  const sizes = [8, 16, 32, 64, 128, 256, 512, 1024, 2048];
  const asio = { sample_rate: 44100, reference_rate: 44100, buffer_size: 512, input_latency: 571, output_latency: 632, buffer_sizes: sizes, safe_mode: true, asio_clients: 1 };
  await page.route("**/api/v1/devices/*/driver*", (route) =>
    route.request().method() === "PUT"
      ? route.fulfill({ status: 409, json: { error: { code: "asio_in_use", message: "1 program is using the driver's ASIO interface." } } })
      : route.fulfill({
          json: {
            device_id: QUADRO, read_at_ms: 1, cached: false, state: "read", dll: "x", service: "s", api_version: "5.12", api_known: true,
            driver_version: { state: "read", value: "5.68.0" }, sample_rate: { state: "read", value: 44100 }, asio_instances: 1, asio_instance: 0,
            asio: { state: "read", value: asio }, safe_mode: { state: "read", value: true },
          },
        }),
  );
  await visit(`devices/${QUADRO}`, "ga-device-status");
  await page.getByTestId("driver-buffer-menu").selectOption("256");
  await page.getByTestId("driver-buffer-confirm").click();
  await expect(page.getByTestId("driver-force")).toBeVisible();
  await page.getByTestId("driver-buffer-menu").selectOption("128");
  await expect(page.getByTestId("driver-buffer-confirm")).toBeVisible();
  await check(page, `devices ${QUADRO} with a driver`);
  await page.unroute("**/api/v1/devices/*/driver*");

  for (const device of [QUADRO, STUDIO]) {
    await visit(`inputs/${device}`, "ga-inputs");
    // A link badge pressed opens the link bar and its buttons.
    await page.getByTestId("pre-link-0").click();
    await check(page, `inputs ${device}`);
    await page.getByTestId("link-cancel").click();

    await visit(`outputs/${device}`, "ga-outputs");
    await check(page, `outputs ${device}`);

    await visit(`routing/${device}`, "ga-routing");
    await check(page, `routing ${device}`);
  }

  // The Quadro's mixer with channels, a group, a link and the colour popover open.
  await visit(`mixer/${QUADRO}`, "ga-channel");
  await page.getByTestId("colour-6").click();
  await check(page, `mixer ${QUADRO}`);
  await page.keyboard.press("Escape");
  // A save with no name raises a notice, which has its own controls.
  await page.getByTestId("layout-save").click();
  await expect(page.locator("ga-notices .notice").first()).toBeVisible();
  await check(page, `mixer ${QUADRO} with a notice`);
  // The Studio+'s, with nothing set up: its starting layouts, and its reverb send on Mix 1.
  await visit(`mixer/${STUDIO}`, "ga-channel");
  await check(page, `mixer ${STUDIO}`);

  // Effects, with an editor open on each kind of control the catalogue decodes.
  for (const device of [QUADRO, STUDIO]) {
    await visit(`effects/${device}`, "ga-effects");
    await expect(page.getByTestId("edit-0-0")).toBeVisible();
    await page.getByTestId("edit-0-0").click();
    await expect(page.getByTestId("effect-editor")).toBeVisible();
    await page.waitForTimeout(400);
    await check(page, `effects ${device}, first effect`);
    await page.getByTestId("edit-0-1").click();
    await expect(page.getByTestId("effect-editor")).toBeVisible();
    await page.waitForTimeout(400);
    await check(page, `effects ${device}, second effect`);
  }

  // A surface with a strip of every kind, and the dock showing it on another page.
  await visit("surface/s", "ga-surface-strip");
  await check(page, "surface");
  await visit(`inputs/${QUADRO}`, "ga-inputs");
  await page.getByTestId("dock-source-select").selectOption("s");
  await expect(page.locator("ga-mixer-dock ga-surface-strip").first()).toBeVisible();
  await check(page, "dock showing a surface");
  await page.getByTestId("dock-source-select").selectOption("");

  // The Workspace page: a snapshot taken, compared and its recall prepared, then a file to import.
  await visit("workspace", "ga-workspace");
  await page.getByTestId("snapshot-new-name").fill("Before");
  await page.getByTestId("snapshot-take").click();
  await expect(page.locator('[data-testid^="snapshot-row-"]').first()).toBeVisible();
  await page.locator('[data-testid^="snapshot-compare-"]').first().click();
  await expect(page.getByTestId("snapshot-recall-prepare")).toBeVisible();
  await page.getByTestId("snapshot-recall-prepare").click();
  await expect(page.getByTestId("snapshot-recall-summary")).toBeVisible();
  await page.getByTestId("workspace-import-file").setInputFiles({ name: "workspace.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify({ version: 1, groups: [], links: [], aliases: {}, mixers: {} })) });
  await page.getByTestId("workspace-import").click();
  await expect(page.getByTestId("workspace-import-confirm")).toBeVisible();
  await check(page, "workspace");

  // The Aggregate page, with a reason to fix, a registration to put right, both interfaces'
  // controls, a Confirm showing, and a DAW streaming with a gap and a stall.
  await page.route("**/api/v1/aggregate**", (route) => {
    const calibrate = new URL(route.request().url()).pathname.includes("/aggregate/calibrate");
    if (route.request().method() === "POST") return route.fulfill({ json: calibrate ? { started: true } : { buffer_size: 256, changed: 2, refused: 0, devices: [] } });
    return route.fulfill({ json: calibrate ? CALIBRATION : AGGREGATE_ANSWER });
  });
  await visit("aggregate", "ga-aggregate");
  await page.getByTestId("device-0-buffer").selectOption("512");
  await expect(page.getByTestId("device-0-buffer-confirm")).toBeVisible();
  await check(page, "aggregate");
  await page.unroute("**/api/v1/aggregate**");

  // A lazy page that could not be loaded leaves its message and a Try again in its place.
  await page.route(/routing-page.*\.js$/, (route) => route.abort());
  await page.goto(`${server.url}/#/routing/${QUADRO}`);
  await page.reload();
  await expect(page.getByTestId("page-failed")).toBeVisible();
  await check(page, "a page that failed to load");

  test.info().annotations.push({ type: "coverage", description: `${checked} elements checked, ${keysSeen.size} of ${Object.keys(CATALOGUE).length} keys seen` });
  expect(problems, `${problems.length} of ${checked} elements are unexplained`).toEqual([]);
  expect(checked).toBeGreaterThan(1000);
  expect(keysSeen.size, "the walk reached most of the catalogue").toBeGreaterThan(Object.keys(CATALOGUE).length * 0.8);
});
