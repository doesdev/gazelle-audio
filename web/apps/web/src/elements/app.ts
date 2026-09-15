// <ga-app>: the shell. A header across the top; below it a devices zone on the left, the routed
// page in the centre, and a right zone for meters and the Control Room monitor panel;
// a lower zone is reserved for the mixer (phase 4). It applies the chosen theme to the document
// and marks itself disconnected when the server goes away.

import { h } from "../core/dom.ts";
import { cssProperties } from "../themes/theme.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { followHash, PAGES, route, type Route } from "./router.ts";

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
    const leftToggle = h("button", { class: "collapse", type: "button", "data-testid": "collapse-left", "on:click": () => store.togglePanel("left") });
    const rightToggle = h("button", { class: "collapse", type: "button", "data-testid": "collapse-right", "on:click": () => store.togglePanel("right") });
    this.root.replaceChildren(
      h("ga-header"),
      h(
        "div",
        { class: "zones" },
        h("aside", { class: "zone left", "aria-label": "Devices" }, h("div", { class: "rail" }, leftToggle), h("div", { class: "content" }, h("ga-device-list"))),
        h("main", {}, banner, title, page),
        h(
          "aside",
          { class: "zone right", "aria-label": "Meters and control room" },
          h("div", { class: "rail" }, rightToggle),
          h(
            "div",
            { class: "content" },
            h("ga-section", { heading: "Meter" }, h("p", { class: "placeholder" }, "The main output meter arrives with the mixer.")),
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
    this.watch(() => {
      const current = route.value;
      // Without a device in the address, a page shows the first device it can (for the mixer, the first of known model).
      const first = current.id === undefined && (current.page === "devices" || current.page === "inputs" || current.page === "outputs" || current.page === "mixer" || current.page === "routing") ? store.devices.value.find((d) => current.page === "devices" || d.family !== null)?.id : undefined;
      title.textContent = PAGES.find((p) => p.page === current.page)?.label ?? "";
      page.replaceChildren(pageFor(current, first));
    });
  }
}

function pageFor(current: Route, firstDeviceId: string | undefined): HTMLElement {
  switch (current.page) {
    case "devices": {
      const id = current.id ?? firstDeviceId;
      return id === undefined ? h("p", { class: "placeholder" }, "No devices are connected.") : h("ga-device-status", { "device-id": id });
    }
    case "workspace":
      return h("ga-workspace");
    case "inputs": {
      const id = current.id ?? firstDeviceId;
      return id === undefined ? h("p", { class: "placeholder" }, "No device of known model is connected.") : h("ga-inputs", { "device-id": id });
    }
    case "outputs": {
      const id = current.id ?? firstDeviceId;
      return id === undefined ? h("p", { class: "placeholder" }, "No device of known model is connected.") : h("ga-outputs", { "device-id": id });
    }
    case "mixer": {
      const id = current.id ?? firstDeviceId;
      return id === undefined ? h("p", { class: "placeholder" }, "No device with a known mixer is connected.") : h("ga-mixer", { "device-id": id, mixer: current.sub ?? "0" });
    }
    case "routing": {
      const id = current.id ?? firstDeviceId;
      return id === undefined ? h("p", { class: "placeholder" }, "No device of known model is connected.") : h("ga-routing", { "device-id": id });
    }
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-app": GaApp;
  }
}
