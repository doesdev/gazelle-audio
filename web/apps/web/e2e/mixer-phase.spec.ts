// The Mixer page and the dock with a cable dedicated to the phase measurement, on the loopback.
//
// The Quadro plays USB 1 Play 16 straight to its S/PDIF output, and the Studio+ records S/PDIF In L
// on USB Rec 21: the dedicated path, written as dedicating the cable writes it. The Mixer marks the
// channel on the kept playback channel Phase, as the Routing page does, and a Mixer change that
// would break the path (a channel's input or mix, a send, a mix sent to the cable's output or to the
// kept record channel, a layout, a tidy, a drop on the dock) waits behind a confirm that says why.
// Every command the page sends a device is captured from the event socket, so a guarded change is
// seen to send nothing until it is confirmed. The loopback is not in dry run: it keeps the routing
// it is given, so the Mixer reads where its mixes play.

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
const Q = { SPDIF_OUT: 6, MIX_IN: [8, 9, 10, 11] };
const PREAMP = 0, COM_PLAY = 1, Q_MUTE = 10;
// The Studio+'s USB record group, its S/PDIF input and MUTE.
const S_USB_REC = 6, S_SPDIF_IN = 5, S_MUTE = 11;

const CABLE = "the dedicated S/PDIF cable, Quadro S/PDIF Out 1 and 2 → Studio+ S/PDIF In 1 and 2";
const playsToo = (mix: string) => `USB 1 Play 16 would play to ${mix} as well, and the burst the driver plays into it at the start of every session with it. It is kept for the phase measurement over ${CABLE}`;

/** One destination group's 32 slots, MUTE except where `slots` says, written through the command route. */
async function route(device: string, group: number, slots: Record<number, [number, number]>): Promise<void> {
  const mute = device === "loopback-0" ? Q_MUTE : S_MUTE;
  const pairs = Array.from({ length: 32 }, (_, at) => slots[at] ?? [mute, 0]).flat();
  const response = await fetch(`${server.url}/api/v1/devices/${device}/command/set_routing`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ bank_idx: group, bank_configs: pairs }) });
  expect(response.ok, await response.text()).toBe(true);
}

/**
 * The dedicated path, and a Quadro mixer with Vox on Preamp 1, a channel laid out on the kept
 * playback channel (which the device plays Preamp 2 into instead, so Tidy would put it back), and a
 * spare channel in Monitors with no input yet. A saved layout puts a channel on the kept channel too.
 */
async function dedicated(): Promise<void> {
  for (let group = 0; group < 12; group += 1) await route("loopback-0", group, {});
  await route("loopback-0", Q.SPDIF_OUT, { 0: [COM_PLAY, 15] });
  await route("loopback-0", Q.MIX_IN[0] as number, { 6: [PREAMP, 0], 7: [PREAMP, 1] });
  await route("loopback-1", S_USB_REC, { 20: [S_SPDIF_IN, 0] });
  await putWorkspace(server, {
    aliases: { "loopback-0": "Quadro", "loopback-1": "Studio+" },
    cables: [{ id: "c1", from: { device_id: "loopback-0", port: "SPDIF_OUT", first: 0 }, to: { device_id: "loopback-1", port: "SPDIF_IN", first: 0 }, channels: 2, dedicated: { phase_output: 15, phase_input: 20 } }],
    aggregate: {
      callback_master: "Zen Quadro Synergy Core",
      devices: [
        { key: "Zen Quadro Synergy Core", device_id: "loopback-0" },
        { key: "Zen Studio+", device_id: "loopback-1", phase: { master_output: 15, input: 20 } },
      ],
    },
    mixers: {
      "loopback-0": {
        mixes: [{ name: "Monitors" }, { name: "Cue" }],
        groups: [],
        channels: [
          { id: "vox", name: "Vox", slot: 6, sends: [], source: { group: PREAMP, channel: 0 }, main_mix: 0 },
          { id: "kept", name: "Kept", slot: 7, sends: [], source: { group: COM_PLAY, channel: 15 }, main_mix: 0 },
          { id: "spare", name: "Spare", slot: 8, sends: [], main_mix: 0 },
        ],
      },
    },
    layouts: [{ id: "keeps", name: "Keeps", family: "quadro", mixer: { mixes: [], groups: [], channels: [{ id: "k", name: "", slot: 9, source: { group: COM_PLAY, channel: 15 }, main_mix: 1, sends: [] }] } }],
  });
}

/** Every routing write the page sends a device, as the socket carries it: each slot as its hex pair. */
function routings(page: Page): { device_id: string; bank_idx: number; pairs: string[] }[] {
  const sent: { device_id: string; bank_idx: number; pairs: string[] }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", ({ payload }) => {
      const frame = JSON.parse(String(payload)) as { device_id?: string; command?: string; args?: { bank_idx: number; bank_configs: string[] } };
      if (frame.device_id !== undefined && frame.command === "set_routing" && frame.args !== undefined) sent.push({ device_id: frame.device_id, bank_idx: frame.args.bank_idx, pairs: frame.args.bank_configs });
    }),
  );
  return sent;
}

test("the Mixer marks the kept playback channel Phase and asks before a change that breaks the phase path", async ({ page }) => {
  await dedicated();
  const sent = routings(page);
  await page.goto(`${server.url}/#/mixer/loopback-0`);

  // The channel on the kept playback channel is marked, with the cable in its tooltip; Vox is not.
  const badge = page.getByTestId("phase-7");
  await expect(badge).toBeVisible({ timeout: 15_000 });
  await expect(badge).toHaveText("Phase");
  await expect(badge).toHaveAttribute("title", `Kept for the phase measurement over ${CABLE}: it plays to S/PDIF Out L and nowhere else, and is hidden from your DAW.`);
  await expect(page.getByTestId("phase-6")).toBeHidden();
  // The Input menu says which input is kept.
  await expect(page.getByTestId("in-8").locator('option[value="1:15"]')).toHaveText("USB 1 Play 16 (phase)");
  await expect(page.getByTestId("in-8").locator('option[value="1:14"]')).toHaveText("USB 1 Play 15");

  // Choosing it on a channel in Monitors waits behind Confirm, saying why, and sends nothing.
  await page.getByTestId("in-8").selectOption("1:15");
  const confirm = page.getByTestId("in-confirm-8");
  await expect(confirm).toBeVisible();
  await expect(confirm).toHaveAttribute("title", playsToo("Monitors"));
  await page.waitForTimeout(300);
  expect(sent).toEqual([]);
  await confirm.click();
  await expect.poll(() => sent.length).toBe(1);
  expect(sent[0]?.device_id).toBe("loopback-0");
  expect(sent[0]?.bank_idx).toBe(Q.MIX_IN[0]);
  expect(sent[0]?.pairs[8], "slot 9 of Monitors plays USB 1 Play 16").toBe("010f");
  await expect(page.getByTestId("phase-8")).toBeVisible();
  sent.length = 0;

  // Sending the kept channel to Cue as well takes two clicks, the reason in the title.
  await page.getByTestId("mix-1").click();
  await page.getByTestId("show-all-channels").click();
  const inMix = page.getByTestId("in-mix-7");
  await expect(inMix).toHaveText("Add to Cue");
  await expect(inMix).toHaveAttribute("title", `${playsToo("Cue")}. Click twice to do it anyway`);
  await inMix.click();
  await expect(inMix).toHaveText("Confirm");
  await page.waitForTimeout(300);
  expect(sent).toEqual([]);
  await page.getByTestId("mix-0").click();

  // The cable's output is marked in the + Output menu, and sending Monitors there waits for Confirm.
  const add = page.getByTestId("mix-add-output-0");
  await expect(add.locator(`option[value="${Q.SPDIF_OUT}:0"]`)).toHaveText(/ \(phase\)$/);
  await add.selectOption(`${Q.SPDIF_OUT}:0`);
  const outputConfirm = page.getByTestId("mix-output-confirm-0");
  await expect(outputConfirm).toBeVisible();
  await expect(outputConfirm).toHaveAttribute("title", `S/PDIF Out L would stop playing USB 1 Play 16, so the phase measurement over ${CABLE} would hear nothing`);
  await page.waitForTimeout(300);
  expect(sent).toEqual([]);

  // A saved layout with a channel on the kept channel asks first, and says why.
  await page.getByTestId("profile-select").selectOption("saved:keeps");
  const apply = page.getByTestId("profile-apply");
  await apply.click();
  await expect(apply).toHaveText("Confirm");
  await expect(apply).toHaveAttribute("title", `${playsToo("Cue")}. Click twice to apply it anyway.`);
  expect(sent).toEqual([]);

  // Tidy would put the kept channel back on its slot, and its confirm says that breaks the path.
  await page.getByTestId("mix-tidy").click();
  await expect(page.getByTestId("mix-tidy-phase")).toContainText(`This breaks the phase path of a dedicated cable:${playsToo("Monitors")}.`);
  await page.getByTestId("mix-tidy-cancel").click();
  expect(sent).toEqual([]);
});

test("on the follower the kept record channel is marked in the + Output menu and guarded", async ({ page }) => {
  await dedicated();
  const sent = routings(page);
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const add = page.getByTestId("mix-add-output-0");
  const kept = add.locator(`option[value="${S_USB_REC}:20"]`);
  await expect(kept).toHaveText(/21\/22 \(phase\)$/, { timeout: 15_000 });
  await expect(add.locator(`option[value="${S_USB_REC}:18"]`)).not.toHaveText(/phase/);
  await add.selectOption(`${S_USB_REC}:20`);
  const confirm = page.getByTestId("mix-output-confirm-0");
  await expect(confirm).toHaveAttribute("title", `USB Rec 21 would stop recording S/PDIF In L, so the phase measurement over ${CABLE} would hear nothing`);
  await page.waitForTimeout(300);
  expect(sent).toEqual([]);
  // Another record pair goes at once, and leaves the kept record channel as it was.
  await add.selectOption(`${S_USB_REC}:18`);
  await expect(confirm).toBeHidden();
  await expect.poll(() => sent.length).toBeGreaterThan(0);
  for (const one of sent) {
    expect([one.device_id, one.bank_idx]).toEqual(["loopback-1", S_USB_REC]);
    expect(one.pairs[20], "USB Rec 21 still records S/PDIF In L").toBe("0500");
  }
});

test("the dock marks the kept channel too, and a drop of it asks first", async ({ page }) => {
  await dedicated();
  const sent = routings(page);
  await page.goto(`${server.url}/#/routing/loopback-0`);
  const dock = page.locator("ga-mixer-dock");
  await expect(dock.getByTestId("phase-7")).toBeVisible({ timeout: 15_000 });
  await expect(dock.getByTestId("phase-6")).toBeHidden();
  // The Routing page marks the same channel.
  await expect(page.getByTestId("source-1-15")).toHaveAttribute("data-phase", "");

  await page.getByTestId("source-1-15").dragTo(dock);
  // Kept is on the same input already, so the drop would double it as well, which is said after.
  await expect(dock.getByTestId("dock-drop-reason")).toHaveText(`Monitors: ${playsToo("Monitors")}. USB 1 Play 16 is in this mix twice, so it is summed twice (about +6 dB)`);
  await expect(dock.getByTestId("dock-drop-confirm")).toBeVisible();
  await dock.getByTestId("dock-drop-cancel").click();
  expect(sent).toEqual([]);
});
