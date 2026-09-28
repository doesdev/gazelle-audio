// A cable dedicated to the aggregate's phase measurement, end to end on the loopback.
//
// The server is the loopback, not in dry run, so it keeps the routing it is given and remembers every
// write, which is what its readiness answer reads. The aggregate runs on the server's own fakes
// (`aggregateFakes`): a registry listing both models' drivers and Gazelle Aggregate, and nothing of
// this machine's, so the answer, and its reasons, are the server's own and not answered in the
// browser. Every command the page sends a device is captured from the event socket and checked
// exactly, because a routing write that is nearly right is a burst in somebody's monitors.
//
// Set GAZELLE_PHASE_SHOTS to a folder to keep a screenshot of each step there.

import { expect, test, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "200"], { webUi: true, aggregateFakes: true });
});

test.afterAll(async () => {
  await server?.stop();
});

// The Quadro's destinations and sources by place, as its topology lists them.
const Q = { LINE_OUT: 0, HP1: 1, HP2: 2, MONITOR: 3, COM_REC: 4, USB_B_REC: 5, SPDIF_OUT: 6, AFX_IN: 7, MIX_IN: [8, 9, 10, 11] };
const COM_PLAY = 1, MIX1_OUT = 6, Q_MUTE = 10;
// The Studio+'s USB record group, its S/PDIF input and MUTE.
const S_USB_REC = 6, S_SPDIF_IN = 5, S_MUTE = 11;

const hex = (source: number, channel: number) => [source, channel].map((b) => b.toString(16).padStart(2, "0")).join("");

/** One destination group's 32 slots, MUTE except where `slots` says, written through the command route. */
async function route(device: string, group: number, slots: Record<number, [number, number]>): Promise<void> {
  const mute = device === "loopback-0" ? Q_MUTE : S_MUTE;
  const pairs = Array.from({ length: 32 }, (_, at) => slots[at] ?? [mute, 0]).flat();
  const response = await fetch(`${server.url}/api/v1/devices/${device}/command/set_routing`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ bank_idx: group, bank_configs: pairs }) });
  expect(response.ok, await response.text()).toBe(true);
}

/**
 * The owner's PC. On the Quadro, USB 1 PLAY 1 and 2 go into Mix 1, USB 1 PLAY 3 into Mix 1 and Mix 4
 * for a headphone amp, Mix 1 to HP1, and the S/PDIF output is muted. The Studio+ records S/PDIF in L
 * on USB REC 21. Its phase is still set up over USB 1 PLAY 3 and USB REC 21, with a reference.
 */
async function ownersPc(): Promise<void> {
  for (let group = 0; group < 12; group += 1) await route("loopback-0", group, {});
  await route("loopback-0", Q.MIX_IN[0] as number, { 16: [COM_PLAY, 0], 17: [COM_PLAY, 1], 4: [COM_PLAY, 2] });
  await route("loopback-0", Q.MIX_IN[3] as number, { 9: [COM_PLAY, 2] });
  await route("loopback-0", Q.HP1, { 0: [MIX1_OUT, 0], 1: [MIX1_OUT, 1] });
  await route("loopback-1", S_USB_REC, { 20: [S_SPDIF_IN, 0] });
  await putWorkspace(server, {
    aliases: { "loopback-0": "Quadro", "loopback-1": "Studio+" },
    cables: [{ id: "c1", from: { device_id: "loopback-0", port: "SPDIF_OUT", first: 0 }, to: { device_id: "loopback-1", port: "SPDIF_IN", first: 0 }, channels: 2 }],
    aggregate: {
      callback_master: "Zen Quadro Synergy Core",
      devices: [
        { key: "Zen Quadro Synergy Core", device_id: "loopback-0" },
        { key: "Zen Studio+", device_id: "loopback-1", phase: { master_output: 2, input: 20, reference: -37 } },
      ],
    },
  });
}

/** Every command the page sends a device, reads left out, as the socket carries it. */
function capture(page: Page): { device_id: string; command: string; args?: unknown }[] {
  const sent: { device_id: string; command: string; args?: unknown }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", ({ payload }) => {
      const frame = JSON.parse(String(payload)) as { device_id?: string; command?: string; args?: unknown };
      if (frame.device_id !== undefined && frame.command !== undefined && !frame.command.startsWith("get_")) sent.push({ device_id: frame.device_id, command: frame.command, ...(frame.args === undefined ? {} : { args: frame.args }) });
    }),
  );
  return sent;
}

async function shot(page: Page, name: string, target?: string, around?: { left: number; up: number; width: number; height: number }): Promise<void> {
  const folder = process.env["GAZELLE_PHASE_SHOTS"];
  if (folder === undefined || folder === "") return;
  const path = `${folder}/${name}.png`;
  if (target === undefined) return void (await page.screenshot({ path }));
  const element = page.getByTestId(target);
  if (around === undefined) return void (await element.screenshot({ path }));
  await element.scrollIntoViewIfNeeded();
  const box = await element.boundingBox();
  if (box !== null) await page.screenshot({ path, clip: { x: Math.max(0, box.x - around.left), y: Math.max(0, box.y - around.up), width: around.width, height: around.height } });
}

const reason = (page: Page) => page.getByTestId("reason-phase_path_broken");

/** Opens one of a card's parts, which stays open for the tab once opened. */
async function open(page: Page, part: string): Promise<void> {
  if ((await page.getByTestId(`${part}-part`).getAttribute("open")) === null) await page.getByTestId(`${part}-open`).click();
}

test("a cable dedicated to the phase measurement routes its path once, is guarded, and is put back when broken", async ({ page }) => {
  await ownersPc();
  const sent = capture(page);
  const routings = () => sent.filter((one) => one.command === "set_routing");

  // The owner's case, as the server's readiness says it, with nothing to press but the way to the cables.
  await page.goto(`${server.url}/#/aggregate`);
  await expect(reason(page)).toContainText("The phase path over the S/PDIF cable is broken: Quadro's S/PDIF out L is muted, not USB 1 PLAY 3; USB 1 PLAY 3 also goes to Mix 1 and Mix 4, and the short burst the driver plays into it at the start of every session plays wherever it goes.", { timeout: 15_000 });
  await expect(page.getByTestId("reason-severity-phase_path_broken")).toHaveText("WORTH KNOWING");
  await expect(page.getByTestId("reason-fix-phase_path_broken")).toHaveCount(0);
  await expect(page.getByTestId("reason-page-phase_path_broken")).toHaveText("Open the Workspace page");
  await shot(page, "1-readiness-owners-case", "aggregate-reasons");
  // USB 1 PLAY 3 is kept for the measurement, hidden from the DAW, and the list says so.
  await open(page, "device-0-channels");
  await expect(page.getByTestId("device-0-out-2-kept")).toHaveText("Kept for the phase measurement, hidden from your DAW");
  await expect(page.getByTestId("device-0-out-2-expose")).toHaveText("Kept");

  // The setting, on the cable: offered, then the confirm lists exactly what will be written.
  await page.getByTestId("reason-page-phase_path_broken").click();
  const dedicate = page.getByTestId("cable-dedicate-c1");
  await expect(dedicate).toHaveText("Dedicate to phase and clock");
  await expect(dedicate).toBeEnabled();
  await dedicate.click();
  const lines = page.getByTestId("cable-dedicate-lines-c1").locator("li");
  await expect(lines.nth(0)).toHaveText("On Quadro, S/PDIF out L plays USB 1 PLAY 16 directly, with no mix between (it is muted now).");
  await expect(lines.nth(1)).toHaveText("On Studio+, USB REC 21 already records S/PDIF in L, so it stays as it is.");
  await expect(lines.nth(2)).toHaveText("In the aggregate's setup, Studio+'s phase is measured from Quadro's USB 1 PLAY 16 to its USB REC 21 (it was USB 1 PLAY 3 to USB REC 21).");
  await expect(lines.nth(3)).toContainText("Its phase reference, -37 samples, is taken out");
  await expect(lines.nth(4)).toHaveText("USB 1 PLAY 16 and USB REC 21 are kept for the phase measurement and hidden from your DAW. USB 1 PLAY 3 is given back to it.");
  await shot(page, "2-cable-setting-confirm", "cable-row-c1");
  expect(routings(), "the confirm writes nothing").toEqual([]);

  // Dedicating writes the S/PDIF output's group once, every other slot as it was, and nothing on the Studio+.
  await page.getByTestId("cable-dedicate-apply-c1").click();
  await expect.poll(routings).toEqual([{ device_id: "loopback-0", command: "set_routing", args: { bank_idx: Q.SPDIF_OUT, bank_configs: [hex(COM_PLAY, 15), ...Array.from({ length: 31 }, () => hex(Q_MUTE, 0))] } }]);
  await expect(page.getByTestId("cable-dedicated-c1")).toHaveText("Dedicated to phase and clock: Quadro USB 1 PLAY 16 → S/PDIF out L → Studio+ USB REC 21");
  await expect(page.getByTestId("cable-dedicate-status-c1")).toHaveText("Dedicated to the phase measurement and the clock. Measure the interfaces again on the Aggregate page to give the phase a reference.");
  await expect(page.getByTestId("cable-dedicate-c1")).toHaveText("Turn off");
  await shot(page, "3-cable-dedicated", "cable-row-c1");
  const saved = async () => (await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { cables: { dedicated?: unknown }[]; aggregate: { devices: { phase?: unknown }[] } };
  await expect.poll(async () => (await saved()).cables[0]?.dedicated).toEqual({ phase_output: 15, phase_input: 20 });
  expect((await saved()).aggregate.devices[1]?.phase, "the reference went with the old path").toEqual({ master_output: 15, input: 20 });

  // The Aggregate page's phase setup names the cable's channels, and the path is whole.
  await page.goto(`${server.url}/#/aggregate`);
  await open(page, "device-1-phase");
  await expect(page.getByTestId("device-1-phase-leaves")).toHaveValue("15");
  await expect(page.getByTestId("device-1-phase-arrives")).toHaveValue("20");
  await expect(page.getByTestId("device-1-phase-summary")).toHaveText("Set up, no reference yet");
  await expect(page.getByTestId("device-1-phase-dedicated")).toContainText("These are the channels of the dedicated S/PDIF cable, Quadro S/PDIF out 1 and 2 → Studio+ S/PDIF in 1 and 2.");
  await page.getByTestId("aggregate-refresh").click();
  await expect(page.getByTestId("aggregate-reasons")).not.toContainText("phase path", { timeout: 10_000 });
  await open(page, "device-0-channels");
  await expect(page.getByTestId("device-0-out-15-kept")).toBeVisible();
  await expect(page.getByTestId("device-0-out-2-expose")).toHaveText("On", { timeout: 5000 });

  // The Routing page marks the kept channels, and breaking the path asks first, naming the cable.
  await page.goto(`${server.url}/#/routing/loopback-0`);
  const cell = page.getByTestId(`dest-${Q.SPDIF_OUT}-0`);
  await expect(cell).toHaveAttribute("data-phase", "");
  await expect(cell).toHaveAttribute("title", /Kept for the phase measurement over the dedicated S\/PDIF cable, Quadro S\/PDIF out 1 and 2 → Studio\+ S\/PDIF in 1 and 2: it plays USB 1 PLAY 16 and nothing else\./);
  await expect(page.getByTestId(`source-${COM_PLAY}-15`)).toHaveAttribute("data-phase", "");
  await expect(page.getByTestId(`dest-${Q.SPDIF_OUT}-1`)).not.toHaveAttribute("data-phase", "");
  await shot(page, "4-routing-badges");
  await shot(page, "4a-routing-badge-cell", `dest-${Q.SPDIF_OUT}-0`, { left: 220, up: 80, width: 520, height: 150 });
  await shot(page, "4b-routing-badge-chip", `source-${COM_PLAY}-15`, { left: 480, up: 20, width: 560, height: 70 });
  const before = routings().length;
  await page.getByTestId(`source-${MIX1_OUT}-0`).click();
  await cell.click();
  const confirm = page.getByTestId("routing-phase-confirm");
  await expect(confirm).toBeVisible();
  await expect(page.getByTestId("routing-phase-confirm-lines")).toHaveText("S/PDIF out L would stop playing USB 1 PLAY 16, so the phase measurement over the dedicated S/PDIF cable, Quadro S/PDIF out 1 and 2 → Studio+ S/PDIF in 1 and 2 would hear nothing.");
  await shot(page, "5-routing-confirm");
  await page.waitForTimeout(300);
  expect(routings().length, "asking writes nothing").toBe(before);
  await page.getByTestId("routing-phase-confirm-apply").click();
  await expect.poll(() => routings().slice(before)).toEqual([{ device_id: "loopback-0", command: "set_routing", args: { bank_idx: Q.SPDIF_OUT, bank_configs: [hex(MIX1_OUT, 0), ...Array.from({ length: 31 }, () => hex(Q_MUTE, 0))] } }]);
  await expect(confirm).toBeHidden();

  // The readiness says so, and its fix puts the path back after a second click.
  await page.goto(`${server.url}/#/aggregate`);
  await expect(reason(page)).toHaveText(
    "The phase path over the S/PDIF cable is broken: Quadro's S/PDIF out L no longer plays USB 1 PLAY 16: it plays Mix 1 L now. Until it is back the measurement hears nothing, and each session is lined up by the figures the drivers report. The S/PDIF cable is dedicated to it, so it can be put back from here.",
    { timeout: 15_000 },
  );
  const fix = page.getByTestId("reason-fix-phase_path_broken");
  await expect(fix).toHaveText("Put the phase path back");
  await shot(page, "6-readiness-broken-dedicated", "aggregate-reasons");
  const broken = routings().length;
  await fix.click();
  await expect(fix).toHaveText("Confirm");
  await page.waitForTimeout(300);
  expect(routings().length, "one click writes nothing").toBe(broken);
  await fix.click();
  await expect.poll(() => routings().slice(broken)).toEqual([{ device_id: "loopback-0", command: "set_routing", args: { bank_idx: Q.SPDIF_OUT, bank_configs: [hex(COM_PLAY, 15), ...Array.from({ length: 31 }, () => hex(Q_MUTE, 0))] } }]);
  await expect(page.getByTestId("aggregate-outcome")).toHaveText("The phase path is back: one routing group was sent.");
  await expect(reason(page)).toHaveCount(0, { timeout: 15_000 });
});

test("a cable that cannot carry the phase is offered greyed, with the reason", async ({ page }) => {
  await ownersPc();
  await putWorkspace(server, {
    aliases: { "loopback-0": "Quadro", "loopback-1": "Studio+" },
    cables: [{ id: "back", from: { device_id: "loopback-1", port: "SPDIF_OUT", first: 0 }, to: { device_id: "loopback-0", port: "SPDIF_IN", first: 0 }, channels: 2 }],
    aggregate: { callback_master: "Zen Quadro Synergy Core", devices: [{ key: "Zen Quadro Synergy Core", device_id: "loopback-0" }, { key: "Zen Studio+", device_id: "loopback-1" }] },
  });
  await page.goto(`${server.url}/#/workspace`);
  await expect(page.getByTestId("cable-dedicate-back")).toBeDisabled();
  await expect(page.getByTestId("cable-dedicate-why-back")).toHaveText("This cable runs into Quadro, which drives the callback. A phase is measured from the callback master to another interface, so only a cable leaving Quadro can carry it.");
});

test("turning a dedication off leaves the routing, and keeps or clears the phase setup as asked", async ({ page }) => {
  await ownersPc();
  await route("loopback-0", Q.SPDIF_OUT, { 0: [COM_PLAY, 15] });
  await putWorkspace(server, {
    aliases: { "loopback-0": "Quadro", "loopback-1": "Studio+" },
    cables: [{ id: "c1", from: { device_id: "loopback-0", port: "SPDIF_OUT", first: 0 }, to: { device_id: "loopback-1", port: "SPDIF_IN", first: 0 }, channels: 2, dedicated: { phase_output: 15, phase_input: 20 } }],
    aggregate: { callback_master: "Zen Quadro Synergy Core", devices: [{ key: "Zen Quadro Synergy Core", device_id: "loopback-0" }, { key: "Zen Studio+", device_id: "loopback-1", phase: { master_output: 15, input: 20 } }] },
  });
  const sent = capture(page);
  await page.goto(`${server.url}/#/workspace`);
  await page.getByTestId("cable-dedicate-c1").click();
  await page.getByTestId("cable-release-keep-c1").click();
  await expect(page.getByTestId("cable-dedicate-c1")).toHaveText("Dedicate to phase and clock");
  const saved = async () => (await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { cables: { dedicated?: unknown }[]; aggregate: { devices: { phase?: unknown }[] } };
  await expect.poll(async () => (await saved()).cables[0]?.dedicated).toBeUndefined();
  expect((await saved()).aggregate.devices[1]?.phase).toEqual({ master_output: 15, input: 20 });
  expect(sent.filter((one) => one.command === "set_routing"), "turning it off writes no routing").toEqual([]);

  // A dedication means nothing once its follower's phase setup is cleared, or the callback master
  // changes (which the server refuses while the new master has a phase setup of its own, so the
  // phase setup goes first), and both pages say so.
  await putWorkspace(server, {
    aliases: { "loopback-0": "Quadro", "loopback-1": "Studio+" },
    cables: [{ id: "c1", from: { device_id: "loopback-0", port: "SPDIF_OUT", first: 0 }, to: { device_id: "loopback-1", port: "SPDIF_IN", first: 0 }, channels: 2, dedicated: { phase_output: 15, phase_input: 20 } }],
    aggregate: { callback_master: "Zen Quadro Synergy Core", devices: [{ key: "Zen Quadro Synergy Core", device_id: "loopback-0" }, { key: "Zen Studio+", device_id: "loopback-1" }] },
  });
  // The page keeps the workspace it loaded, so it is loaded again to see the one just put.
  await page.reload();
  await expect(page.getByTestId("cable-dedicated-stale-c1")).toContainText("Studio+ has no phase setup now");
  await page.goto(`${server.url}/#/aggregate`);
  await page.getByTestId("aggregate-master").selectOption("Zen Studio+");
  await expect(page.getByTestId("aggregate-master-dedication")).toContainText("Quadro does not drive the callback now, and a phase is measured from the interface that does");
  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { aggregate: { callback_master?: string } }).aggregate.callback_master).toBe("Zen Studio+");
  await page.getByTestId("aggregate-refresh").click();
  await expect(page.getByTestId("reason-phase_dedication_stale")).toContainText("The S/PDIF cable from Quadro to Studio+ is dedicated to the phase measurement, and Quadro does not drive the callback now", { timeout: 15_000 });
  await expect(page.getByTestId("reason-page-phase_dedication_stale")).toHaveText("Open the Workspace page");

  await page.goto(`${server.url}/#/workspace`);
  await expect(page.getByTestId("cable-dedicated-stale-c1")).toContainText("Quadro does not drive the callback now");
  await page.getByTestId("cable-dedicate-c1").click();
  await page.getByTestId("cable-release-clear-c1").click();
  await expect.poll(async () => (await saved()).cables[0]?.dedicated).toBeUndefined();
  expect(sent.filter((one) => one.command === "set_routing"), "nothing here writes routing").toEqual([]);
});
