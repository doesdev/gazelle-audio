// <ga-device-list>: every connected device as a card, by the name the user gave it, with a colour
// swatch from the theme's channel palette and its state at a glance from its status report: the
// clock it runs at and whether it is locked, power, the preset it is on, and whether any hardware
// input has signal or has clipped.
//
// Picking a card switches the page you are on to that device (the user, 2026-09-16), so the pages
// need no device picker of their own. On the Workspace page, which shows no one device, it selects
// the device without leaving, and the next page with a device opens on it. The card marked is the
// device the page shows, or on the Workspace page the one selected.

import { h } from "../core/dom.ts";
import { effect, untracked } from "../core/signal.ts";
import { displayName, SAMPLE_RATES, type DeviceDescriptor } from "../store/store.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href, route, type Page } from "./router.ts";

const STATUS_REPORT = "0x73";

/** The pages that show one device, and so can switch to another in place. */
const DEVICE_PAGES: readonly Page[] = ["devices", "inputs", "outputs", "mixer", "routing"];

export class GaDeviceList extends GaElement {
  static override styles = [
    sheet(`
      ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
      a {
        display: grid;
        grid-template-columns: 4px minmax(0, 1fr) auto;
        grid-template-areas: "swatch name input" "swatch clock clock" "swatch state state";
        column-gap: 8px;
        row-gap: 1px;
        padding: 5px 8px 5px 0;
        border-radius: 3px;
        color: inherit;
        text-decoration: none;
      }
      a:hover { background: var(--ga-control-hover); }
      a[aria-current="page"] { background: var(--ga-control-active); }
      a[aria-disabled="true"] { opacity: 0.55; cursor: not-allowed; }
      .swatch { grid-area: swatch; align-self: stretch; border-radius: 2px; background: var(--swatch); }
      .name { grid-area: name; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 600; }
      .clock, .state { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; font-size: 11px; color: var(--ga-text-secondary); font-variant-numeric: tabular-nums; }
      .clock { grid-area: clock; }
      .state { grid-area: state; }
      .lock { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-muted); background: var(--ga-surface-inset); }
      .lock[data-locked] { color: var(--ga-text-inverse); background: var(--ga-accent); }
      .power[data-standby] { color: var(--ga-state-mute); }
      .input { grid-area: input; align-self: center; width: 8px; height: 14px; border-radius: 2px; background: var(--ga-surface-inset); border: 1px solid var(--ga-border-subtle); }
      .input[data-level="signal"] { background: var(--ga-accent); border-color: transparent; }
      .input[data-level="clip"] { background: var(--ga-state-mute); border-color: transparent; }
      .unknown { grid-area: clock; font-size: 11px; color: var(--ga-text-muted); }
      .empty { padding: 8px; color: var(--ga-text-muted); }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const list = h("ul", { "aria-label": "Devices" });
    this.root.replaceChildren(list);

    // The cards are rebuilt when the devices, their names or the palette change. Each card's status
    // follows its own report, in effects that go with the cards they fill.
    this.watch(() => {
      const devices = store.devices.value;
      const workspace = store.workspace.value;
      const colours = Math.max(1, store.theme.value.palette.length);
      const disposers: (() => void)[] = [];
      list.replaceChildren(
        ...(devices.length === 0
          ? [h("li", { class: "empty" }, "No devices")]
          : // Untracked: building a card starts its report and effects, and none of that should make
            // this watch rebuild every card on every report.
            untracked(() => devices.map((device, i) => h("li", {}, this.#card(device, displayName(device, workspace), i % colours, disposers))))),
      );
      return () => {
        for (const dispose of disposers) dispose();
      };
    });
  }

  #card(device: DeviceDescriptor, name: string, colour: number, disposers: (() => void)[]): HTMLElement {
    const store = useStore();
    const card = h(
      "a",
      {
        "data-device-id": device.id,
        "on:click": (event: Event) => {
          // The Workspace page shows no one device: select it and stay. A page that needs a device of
          // known model cannot show one of unknown model, so its card does nothing there.
          if (card.getAttribute("aria-disabled") === "true") event.preventDefault();
          else if (route.peek().page === "workspace") {
            event.preventDefault();
            store.selectDevice(device.id);
          }
        },
      },
      h("span", { class: "swatch", style: `--swatch: var(--ga-channel-palette-${colour})` }),
      h("span", { class: "name", title: device.model ?? device.id }, name),
    );

    // Where the card leads, and whether it is the device on screen, follow the page.
    disposers.push(
      effect(() => {
        const current = route.value;
        const page = current.page;
        const known = page !== "devices" && page !== "workspace";
        const usable = !known || device.family !== null;
        card.setAttribute("aria-disabled", String(!usable));
        card.title = usable ? "" : "This device's model is unknown, so only its Devices page can show it";
        card.setAttribute("href", DEVICE_PAGES.includes(page) ? href({ page, id: device.id }) : href({ page }));
        const shown = page === "workspace" ? store.selectedDevice.value : (current.id ?? store.deviceInView(known));
        if (shown === device.id) card.setAttribute("aria-current", "page");
        else card.removeAttribute("aria-current");
      }),
    );

    if (device.family === null) {
      card.append(h("span", { class: "unknown" }, "Unknown model"));
      return card;
    }

    const rate = h("span", { class: "rate" }, "—");
    const lock = h("span", { class: "lock" }, "NO LOCK");
    const power = h("span", { class: "power" }, "—");
    const preset = h("span", { class: "preset" }, "—");
    const input = h("span", { class: "input", "data-level": "quiet", role: "img", "aria-label": "Inputs: quiet" });
    card.append(input, h("span", { class: "clock" }, rate, lock), h("span", { class: "state" }, power, h("span", { "aria-hidden": "true" }, "·"), preset));

    disposers.push(store.watchReport(device.id, STATUS_REPORT));
    disposers.push(
      effect(() => {
        const state = store.deviceCard(device.id);
        if (state === undefined) return;
        const clock = state.clock;
        // The measured rate while there is one, else the rate the device is set to.
        rate.textContent = clock === undefined ? "—" : clock.hz > 0 ? `${(clock.hz / 1000).toFixed(1)} kHz` : (SAMPLE_RATES[clock.rate] ?? "—");
        lock.textContent = clock?.locked === true ? "LOCKED" : "NO LOCK";
        lock.toggleAttribute("data-locked", clock?.locked === true);
        power.textContent = state.power === undefined ? "—" : state.power ? "On" : "Standby";
        power.toggleAttribute("data-standby", state.power === false);
        preset.textContent = state.preset === undefined ? "—" : `Preset ${state.preset}`;
        input.setAttribute("data-level", state.input);
        input.setAttribute("aria-label", `Inputs: ${state.input === "quiet" ? "quiet" : state.input === "signal" ? "signal" : "clipped"}`);
      }),
    );
    return card;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-device-list": GaDeviceList;
  }
}
