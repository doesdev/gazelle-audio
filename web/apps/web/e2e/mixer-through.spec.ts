// Channels named through a cable, on the loopback (not in dry run, so routing reads answer). The
// Studio+ plays Preamp 1 (its channel Kick) and USB Play 3 down its ADAT Out 1 and 2 and nothing
// down 3; a declared cable takes ADAT Out 1 to 8 into the Quadro's ADAT In 1 to 8. The Quadro's
// Mixer names its unnamed channels on ADAT In after what the Studio+ sends, keeps a name the user
// gave, bands each run of side-by-side cable channels "From Live room (ADAT)", and its Input menu
// says what each ADAT input carries. The dock shows the same names.

import { expect, test } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "200"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

// The Quadro's sources and its first mix's inputs by place, as its topology lists them.
const Q = { PREAMP: 0, ADAT_IN: 3, MUTE: 10, MIX_IN: 8 };
// The Studio+'s ADAT output, the sources it plays there, and MUTE.
const S = { ADAT_OUT: 7, PREAMP: 0, USB_PLAY: 3, MUTE: 11 };

/** One destination group's 32 slots, MUTE except where `slots` says, as a device command. */
async function route(device: string, group: number, slots: Record<number, [number, number]>): Promise<void> {
  const mute = device === "loopback-0" ? Q.MUTE : S.MUTE;
  const pairs = Array.from({ length: 32 }, (_, at) => slots[at] ?? [mute, 0]).flat();
  const response = await fetch(`${server.url}/api/v1/devices/${device}/command/set_routing`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ bank_idx: group, bank_configs: pairs }) });
  expect(response.ok, await response.text()).toBe(true);
}

test("the Quadro's channels on the Studio+'s ADAT cable are named after what it sends, banded, and listed so in the Input menu", async ({ page }) => {
  await route("loopback-1", S.ADAT_OUT, { 0: [S.PREAMP, 0], 1: [S.USB_PLAY, 2] });
  // ADAT In 1, 2 and 3, then Vox on Preamp 1, then ADAT In 4: two runs of cable channels.
  await route("loopback-0", Q.MIX_IN, { 6: [Q.ADAT_IN, 0], 7: [Q.ADAT_IN, 1], 8: [Q.ADAT_IN, 2], 9: [Q.PREAMP, 0], 10: [Q.ADAT_IN, 3] });
  const adat = (channel: number) => ({ group: Q.ADAT_IN, channel });
  await putWorkspace(server, {
    aliases: { "loopback-0": "Desk", "loopback-1": "Live room" },
    cables: [{ id: "c1", from: { device_id: "loopback-1", port: "ADAT_OUT", first: 0 }, to: { device_id: "loopback-0", port: "ADAT_IN", first: 0 }, channels: 8 }],
    mixers: {
      "loopback-1": { mixes: [], groups: [], channels: [{ id: "kick", name: "Kick", slot: 0, color: "#aa3311", sends: [], source: { group: S.PREAMP, channel: 0 }, main_mix: 0 }] },
      "loopback-0": {
        mixes: [{ name: "Monitors" }],
        groups: [],
        channels: [
          { id: "a1", name: "", slot: 6, sends: [], source: adat(0), main_mix: 0 },
          { id: "a2", name: "", slot: 7, sends: [], source: adat(1), main_mix: 0 },
          { id: "a3", name: "", slot: 8, sends: [], source: adat(2), main_mix: 0 },
          { id: "vox", name: "Vox", slot: 9, sends: [], source: { group: Q.PREAMP, channel: 0 }, main_mix: 0 },
          { id: "a4", name: "Bass DI", slot: 10, sends: [], source: adat(3), main_mix: 0 },
        ],
      },
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);

  // Named through: the sender's channel with its colour, a playback channel by its label, and plain
  // where the Studio+ sends nothing. The tooltip says where each comes from.
  const strip = (slot: number) => page.locator(`ga-mixer ga-strip[strip="${slot}"]`);
  await expect(strip(6)).toHaveAttribute("label", "Kick", { timeout: 15_000 });
  await expect(strip(6)).toHaveAttribute("color", "#aa3311");
  await expect(page.locator("ga-mixer").getByTestId("select-6")).toHaveAttribute("title", "Kick (from Live room, ADAT In 1)");
  await expect(page.getByTestId("name-6")).toHaveAttribute("placeholder", "Kick");
  await expect(strip(7)).toHaveAttribute("label", "USB Play 3");
  await expect(strip(8)).toHaveAttribute("label", "ADAT In 3");
  // A name the user gave wins; what the Studio+ sends is in the tooltip.
  await expect(strip(10)).toHaveAttribute("label", "Bass DI");
  await expect(strip(10)).toHaveAttribute("detail", "from Live room, ADAT In 4");
  await expect(strip(9)).toHaveAttribute("label", "Vox");
  await expect(strip(9)).not.toHaveAttribute("detail", /./);

  // Two bands, one per run of side-by-side cable channels; Vox between them is in neither.
  const bands = page.getByTestId("cable-band");
  await expect(bands).toHaveCount(2);
  await expect(bands.first()).toHaveText("From Live room (ADAT)");
  const first = page.locator("ga-mixer ga-cable-band").first();
  await expect(first.locator("ga-channel")).toHaveCount(3);
  await expect(page.locator("ga-mixer ga-cable-band").nth(1).locator("ga-channel")).toHaveCount(1);
  await expect(page.locator('ga-mixer ga-channel[channel-id="vox"]')).toHaveCount(1);
  await expect(page.locator('ga-mixer ga-cable-band ga-channel[channel-id="vox"]')).toHaveCount(0);

  // The Input menu says what each ADAT input carries.
  await expect(page.getByTestId("in-9").locator('option[value="3:0"]')).toHaveText("ADAT In 1: Kick (Live room)");
  await expect(page.getByTestId("in-9").locator('option[value="3:1"]')).toHaveText("ADAT In 2: USB Play 3 (Live room)");
  await expect(page.getByTestId("in-9").locator('option[value="3:2"]')).toHaveText("ADAT In 3");
  await expect(page.getByTestId("in-9").locator('option[value="4:0"]')).toHaveText("S/PDIF In 1");

  // Naming the channel keeps what it carries in the tooltip, live.
  await page.getByTestId("name-6").fill("Bass drum");
  await page.getByTestId("name-6").press("Enter");
  await expect(strip(6)).toHaveAttribute("label", "Bass drum");
  await expect(page.locator("ga-mixer").getByTestId("select-6")).toHaveAttribute("title", "Bass drum (Kick, from Live room, ADAT In 1)");

  // A group of the user's wins over the band: the grouped channel leaves it.
  await page.getByTestId("group-6").selectOption({ label: "New group…" });
  await expect(page.locator('ga-mixer ga-channel-group ga-channel[channel-id="a1"]')).toHaveCount(1);
  await expect(first.locator("ga-channel")).toHaveCount(2);
  await expect(bands).toHaveCount(2);

  // The dock names them the same on another page.
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  const dock = page.locator("ga-mixer-dock");
  await expect(dock.locator('ga-strip[strip="7"]')).toHaveAttribute("label", "USB Play 3", { timeout: 15_000 });
  await expect(dock.locator('ga-strip[strip="7"]')).toHaveAttribute("detail", "from Live room, ADAT In 2");
});
