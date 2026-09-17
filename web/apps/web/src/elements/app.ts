// <ga-app>: the shell. A header across the top; below it a devices zone on the left, the routed
// page in the centre, and a right zone for meters and the Control Room monitor panel;
// a lower zone is reserved for the mixer (phase 4). It applies the chosen theme to the document
// and marks itself disconnected when the server goes away.
//
// A page is built for each change of what is shown: page and device (decision P71). The device
// an address names is remembered as the selected one; an address that names none shows the one
// last selected. Leaving a page disposes of it, so a page that is not shown follows nothing; its
// scroll position and other view state are kept in the store and put back when it is built again.

import { h } from "../core/dom.ts";
import { untracked } from "../core/signal.ts";
import { cssProperties } from "../themes/theme.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { followHash, PAGES, route, type Page } from "./router.ts";
import { keepScroll } from "./view-state.ts";

export class GaApp extends GaElement {
  static override styles = [
    sheet(`
      :host {
        display: grid;
        grid-template-rows: auto 1fr auto;
        height: 100vh;
        background: var(--ga-surface-background);
      }
      .zones {
        --left: minmax(200px, 240px);
        --right: minmax(200px, 260px);
        display: grid;
        grid-template-columns: var(--left) minmax(0, 1fr) var(--right);
        gap: 1px;
        min-height: 0;
      }
      :host([left-collapsed]) .zones { --left: 28px; }
      :host([right-collapsed]) .zones { --right: 28px; }
      .zone, main { min-height: 0; overflow: auto; background: var(--ga-surface-panel); padding: 8px; }
      /* The page fills the rest of main's height, so the mixer can stretch to the window. */
      main { display: flex; flex-direction: column; padding: 12px 16px; }
      .page { display: flex; flex-direction: column; flex: 1 0 auto; }
      aside.zone { display: flex; flex-direction: column; gap: 4px; }
      .rail { display: flex; }
      .right .rail { justify-content: flex-start; }
      .left .rail { justify-content: flex-end; }
      .collapse { min-width: 0; min-height: 20px; padding: 0 6px; font-size: 12px; line-height: 1; color: var(--ga-text-secondary); background: transparent; }
      .collapse:hover { color: var(--ga-text-primary); background: var(--ga-control-hover); }
      :host([left-collapsed]) .left, :host([right-collapsed]) .right { padding: 8px 2px; overflow: hidden; }
      :host([left-collapsed]) .left .content, :host([right-collapsed]) .right .content { display: none; }
      :host([left-collapsed]) .left .rail, :host([right-collapsed]) .right .rail { justify-content: center; }
      .lower { border-top: 1px solid var(--ga-surface-background); padding: 6px 8px; }
      .page-title { margin: 0 0 12px; }
      .disconnected {
        margin: 0 0 12px;
        padding: 6px 10px;
        border-radius: 3px;
        background: var(--ga-connection-reconnecting);
        color: var(--ga-text-inverse);
        font-weight: 600;
      }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    this.onDisconnect(followHash());

    const title = h("h1", { class: "page-title" });
    const banner = h("p", { class: "disconnected", role: "alert", hidden: true }, "The server is not connected. Controls are disabled until it reconnects.");
    const page = h("div", { class: "page" });
    const main = h("main", {}, banner, title, page);
    const leftToggle = h("button", { class: "collapse", type: "button", "data-testid": "collapse-left", "on:click": () => store.togglePanel("left") });
    const rightToggle = h("button", { class: "collapse", type: "button", "data-testid": "collapse-right", "on:click": () => store.togglePanel("right") });
    this.root.replaceChildren(
      h("ga-header"),
      h(
        "div",
        { class: "zones" },
        h("aside", { class: "zone left", "aria-label": "Devices" }, h("div", { class: "rail" }, leftToggle), h("div", { class: "content" }, h("ga-device-list"))),
        main,
        h(
          "aside",
          { class: "zone right", "aria-label": "Meters and control room" },
          h("div", { class: "rail" }, rightToggle),
          h(
            "div",
            { class: "content" },
            h("ga-section", { heading: "Meter" }, h("ga-output-meters")),
            h("ga-section", { heading: "Control Room" }, h("ga-control-room")),
          ),
        ),
      ),
      h("footer", { class: "zone lower", "aria-label": "Mixer" }, h("ga-section", { heading: "Mixer", collapsed: true }, h("p", { class: "placeholder" }, "Open the Mixer page for channel strips, faders and meters. A compact mixer here comes later."))),
      h("ga-notices"),
    );

    this.watch(() => {
      const properties = cssProperties(store.theme.value);
      const style = document.documentElement.style;
      for (const [name, value] of Object.entries(properties)) style.setProperty(name, value);
    });
    this.watch(() => {
      const { leftCollapsed, rightCollapsed } = store.panels.value;
      this.toggleAttribute("left-collapsed", leftCollapsed);
      this.toggleAttribute("right-collapsed", rightCollapsed);
      // The arrows point the way the panel will move.
      leftToggle.textContent = leftCollapsed ? "»" : "«";
      leftToggle.setAttribute("aria-label", leftCollapsed ? "Expand the devices panel" : "Collapse the devices panel");
      leftToggle.setAttribute("aria-expanded", String(!leftCollapsed));
      rightToggle.textContent = rightCollapsed ? "«" : "»";
      rightToggle.setAttribute("aria-label", rightCollapsed ? "Expand the meters panel" : "Collapse the meters panel");
      rightToggle.setAttribute("aria-expanded", String(!rightCollapsed));
    });
    this.watch(() => {
      const connected = store.connected.value;
      banner.hidden = connected;
      this.toggleAttribute("disconnected", !connected);
    });
    let built: string | undefined;
    let scroll: (() => void) | undefined;
    this.onDisconnect(() => scroll?.());
    this.watch(() => {
      const current = route.value;
      title.textContent = PAGES.find((p) => p.page === current.page)?.label ?? "";
      // Every page but the Devices page needs a device of known model.
      const known = current.page !== "devices";
      let deviceId: string | undefined;
      if (current.page !== "workspace") {
        const named = current.id === undefined ? undefined : store.devices.value.find((d) => d.id === current.id);
        deviceId = current.id ?? store.deviceInView(known);
        // A device the address names is the selected one from here on, when this page can show it.
        // That includes one reached by a link rather than a picker: it is still the device on screen.
        if (named !== undefined && (!known || named.family !== null)) untracked(() => store.selectDevice(named.id));
        if (current.page === "mixer" && deviceId !== undefined && current.sub !== undefined) {
          const mix = Number(current.sub);
          untracked(() => store.selectMix(deviceId as string, mix));
        }
      }
      // Only a different page or device is built anew; a new mix is the store's selection, which the
      // Mixer page follows. A page element renders as it is appended, and whatever it reads there
      // would otherwise become a dependency of this effect: the page would be rebuilt on every
      // report, losing anything half-done in it.
      const key = `${current.page}/${deviceId ?? ""}`;
      if (key === built) return;
      built = key;
      untracked(() => {
        scroll?.();
        page.replaceChildren(pageFor(current.page, deviceId));
        scroll = keepScroll(main, store.view(`scroll:${key}`, 0));
      });
    });
  }
}

function pageFor(page: Page, id: string | undefined): HTMLElement {
  switch (page) {
    case "devices":
      return id === undefined ? h("p", { class: "placeholder" }, "No devices are connected.") : h("ga-device-status", { "device-id": id });
    case "workspace":
      return h("ga-workspace");
    case "inputs":
      return id === undefined ? h("p", { class: "placeholder" }, "No device of known model is connected.") : h("ga-inputs", { "device-id": id });
    case "outputs":
      return id === undefined ? h("p", { class: "placeholder" }, "No device of known model is connected.") : h("ga-outputs", { "device-id": id });
    case "mixer":
      return id === undefined ? h("p", { class: "placeholder" }, "No device with a known mixer is connected.") : h("ga-mixer", { "device-id": id });
    case "routing":
      return id === undefined ? h("p", { class: "placeholder" }, "No device of known model is connected.") : h("ga-routing", { "device-id": id });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-app": GaApp;
  }
}
