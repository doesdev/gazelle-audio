// <ga-mixer-dock>: the compact mixer under every page (the user's choice, 2026-09-16), so levels
// can be ridden from Inputs or Routing. It shows the device in view (the page's device if it has a
// known mixer, else the one last selected) in its selected mix, the same per-device mix the Mixer
// page's Mix menu sets, which its own small Mix menu sets too: a slim strip per channel set up in
// that mix, in the Mixer page's order, then the mix's master. Strips scroll sideways; the master
// stays at the right. A mix with no channel set up is one short line pointing to the Mixer page,
// without its master: there is nothing in it to ride, and a fader needs the full row's height.
//
// On the Mixer page it is hidden and builds nothing, since the page shows the same strips in full.
// While hidden or collapsed it follows nothing: the report watch, the mix reads and the strips are
// released, as they are when another device comes into view. Whether it is collapsed is kept per
// browser; until someone chooses, it starts collapsed at phone width, where open it would take a
// quarter of the screen.

import { h } from "../core/dom.ts";
import { effect, untracked } from "../core/signal.ts";
import { meterDeflection } from "../store/mixer.ts";
import { displayName } from "../store/store.ts";
import { meterGradient } from "../themes/theme.ts";
import { channelStrip } from "./channel.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href, route } from "./router.ts";
import type { GaSection } from "./section.ts";

export class GaMixerDock extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; padding: 6px 8px; border-top: 1px solid var(--ga-surface-background); background: var(--ga-surface-panel); }
      :host([hidden]) { display: none; }
      .actions { display: flex; align-items: center; gap: 6px; min-width: 0; }
      .device { font-size: 11px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .actions select { height: 18px; min-height: 18px; padding: 0 4px; font-size: 10px; color: var(--ga-text-secondary); background: transparent; border: 1px solid var(--ga-border-subtle); border-radius: 2px; }
      .actions select:hover { color: var(--ga-text-primary); }
      /* A fixed height that leaves the fader about 130 px of travel. */
      .strips {
        display: flex;
        gap: 2px;
        height: 200px;
        padding: 4px 0 4px 4px;
        overflow-x: auto;
        overflow-y: hidden;
        border-radius: 3px;
        background: var(--ga-surface-inset);
      }
      .strips ga-strip { flex: 0 0 44px; }
      .master { position: sticky; right: 0; display: flex; margin-left: auto; padding: 0 4px 0 6px; background: var(--ga-surface-inset); box-shadow: -8px 0 8px -4px rgb(0 0 0 / 0.5); }
      .master ga-strip { flex: 0 0 48px; }
      /* Nothing to show is one line, not a row of strip height. */
      .strips.empty { height: auto; padding: 0; }
      .empty .placeholder { flex: 1; padding: 4px 8px; font-size: 11px; }
      .empty a { color: var(--ga-accent); }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const device = h("span", { class: "device muted" });
    const mixSelect = h("select", { "aria-label": "Dock mix", "data-testid": "dock-mix-select" });
    const actions = h("div", { class: "actions", slot: "actions" }, device, mixSelect);
    const strips = h("div", { class: "strips", "data-testid": "dock-strips" });
    const section = h("ga-section", { heading: "Mixer" }, actions, strips) as GaSection;
    section.collapsed = store.mixerDockCollapsed.peek();
    section.addEventListener("toggle", () => store.setMixerDockCollapsed(section.collapsed));
    this.root.replaceChildren(section);

    // What the shown device's dock holds on to: its mix reads, meter report and strips. Released
    // when another device is shown, the dock is collapsed or the Mixer page opens.
    let held: (() => void)[] = [];
    const release = () => {
      for (const dispose of held.splice(0)) dispose();
      strips.replaceChildren();
    };
    /** Only a message in the row: it takes one line. */
    const showMessage = (...message: (Node | string)[]) => {
      strips.classList.add("empty");
      strips.replaceChildren(h("p", { class: "placeholder" }, ...message));
    };
    this.onDisconnect(release);
    const follow = (deviceId: string): (() => void)[] => {
      const channels = store.channels(deviceId);
      const own: (() => void)[] = [];
      // Levels come from the device, read once as the Mixer page reads them (P80).
      own.push(
        effect(() => {
          if (store.mixesToRead(deviceId)) untracked(() => void store.readMixes(deviceId));
        }),
      );
      // Strips meter their inputs from the status report, followed while the dock shows the mix.
      own.push(effect(() => store.mixer(deviceId, channels.meteredMix.value).activate()));
      own.push(
        effect(() => {
          const entry = store.devices.value.find((d) => d.id === deviceId);
          device.textContent = entry === undefined ? "" : displayName(entry, store.workspace.value);
          mixSelect.replaceChildren(...Array.from({ length: channels.mixCount }, (_, mix) => h("option", { value: String(mix) }, channels.mixName(mix))));
          mixSelect.value = String(channels.meteredMix.value);
        }),
      );
      const choose = () => {
        channels.meteredMix.value = Number(mixSelect.value);
      };
      mixSelect.addEventListener("change", choose);
      own.push(() => mixSelect.removeEventListener("change", choose));

      // Strips read their attributes once, so the row is rebuilt when what they show changes.
      let rendered = "";
      own.push(
        effect(() => {
          const mix = channels.meteredMix.value;
          const shownStrips = channels.inMix(mix).map((ch) => channelStrip(deviceId, mix, ch.slot, channels.strip(ch, mix), true));
          const mixName = channels.mixName(mix);
          const key = JSON.stringify([mixName, shownStrips]);
          if (key === rendered) return;
          rendered = key;
          // Untracked: a strip renders as it is appended, and what it reads must not rebuild the row.
          untracked(() => {
            if (shownStrips.length === 0) {
              showMessage(`No channel is set up in ${mixName}. `, h("a", { href: href({ page: "mixer", id: deviceId }) }, "Set channels up on the Mixer page."));
              return;
            }
            const master = h("div", { class: "master" }, h("ga-strip", { "device-id": deviceId, mixer: String(mix), strip: "master", label: mixName, compact: "" }));
            strips.classList.remove("empty");
            strips.replaceChildren(...shownStrips.map((attributes) => h("ga-strip", attributes)), master);
          });
        }),
      );
      return own;
    };

    let shown: string | undefined;
    this.watch(() => {
      const current = route.value;
      const onMixer = current.page === "mixer";
      const collapsed = store.mixerDockCollapsed.value;
      const known = store.devices.value.filter((d) => d.family !== null);
      const id = known.find((d) => d.id === current.id)?.id ?? store.deviceInView(true);
      this.hidden = onMixer;
      actions.hidden = id === undefined;
      const key = onMixer || collapsed ? "" : `device:${id ?? ""}`;
      if (key === shown) return;
      shown = key;
      untracked(() => {
        release();
        section.collapsed = collapsed;
        if (key === "") return;
        if (id === undefined) {
          showMessage("No device with a known mixer is connected.");
          return;
        }
        held = follow(id);
      });
    });

    this.watch(() => {
      // Meter gradient stops are dBFS; place them on the scale the meters use, as the Mixer page does.
      this.style.setProperty("--mixer-meter-gradient", meterGradient(store.theme.value.meter.gradient, undefined, "to top", (db) => meterDeflection(-db)));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-mixer-dock": GaMixerDock;
  }
}
