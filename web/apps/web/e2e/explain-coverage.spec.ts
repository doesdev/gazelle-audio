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
  });
}

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
