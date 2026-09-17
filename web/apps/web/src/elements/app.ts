// <ga-app>: the shell. A header across the top; below it the routed page and one sidebar holding
// the device cards, the meter and the Control Room monitor panel; a lower zone is reserved for the
// mixer dock. It applies the chosen theme to the document and marks itself disconnected when the
// server goes away.
//
// The sidebar docks to the right unless moved, and folds to its rail; its side, whether it is
// folded and each section's collapse are remembered per browser (the user, 2026-09-16). At phone
// width it is a drawer over the page instead, opened from the header and closed by Escape, a tap
// outside it, or a new address; it always starts closed there.
//
// A page is built for each change of what is shown: page and device (decision P71). The device
// an address names is remembered as the selected one; an address that names none shows the one
// last selected. Leaving a page disposes of it, so a page that is not shown follows nothing; its
// scroll position and other view state are kept in the store and put back when it is built again.

import { h } from "../core/dom.ts";
import { signal, untracked } from "../core/signal.ts";
import { cssProperties } from "../themes/theme.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import type { SidebarSection } from "../store/store.ts";
import type { GaSection } from "./section.ts";
import { followHash, PAGES, route, type Page } from "./router.ts";
import { keepScroll } from "./view-state.ts";

/** The widest viewport, in CSS pixels, at which the sidebar is a drawer rather than docked. */
export const DRAWER_MAX_PX = 700;

export class GaApp extends GaElement {
  static override styles = [
    sheet(`
      :host {
        display: grid;
        grid-template-rows: auto 1fr auto;
        /* One column no wider than the window, whatever the content's own minimum width. */
        grid-template-columns: minmax(0, 1fr);
        height: 100vh;
        height: 100dvh;
        background: var(--ga-surface-background);
      }
      .zones {
        --sidebar: minmax(220px, 280px);
        display: grid;
        grid-template-columns: minmax(0, 1fr) var(--sidebar);
        grid-template-areas: "main sidebar";
        gap: 1px;
        min-height: 0;
      }
      :host([sidebar-side="left"]) .zones { grid-template-columns: var(--sidebar) minmax(0, 1fr); grid-template-areas: "sidebar main"; }
      :host([sidebar-collapsed]) .zones { --sidebar: 28px; }
      .zone, main { min-width: 0; min-height: 0; overflow: auto; background: var(--ga-surface-panel); padding: 8px; }
      /* The page fills the rest of main's height, so the mixer can stretch to the window. */
      main { grid-area: main; display: flex; flex-direction: column; padding: 12px 16px; }
      .page { display: flex; flex-direction: column; flex: 1 0 auto; min-width: 0; }
      .sidebar { grid-area: sidebar; display: flex; flex-direction: column; gap: 4px; }
      .content { display: grid; gap: 4px; align-content: start; }
      /* The rail: move on the edge towards the page, its arrow pointing across it; fold on the outer edge. */
      .rail { display: flex; align-items: center; gap: 2px; }
      :host([sidebar-side="left"]) .rail { flex-direction: row-reverse; }
      .rail .spacer { flex: 1; }
      .rail button { min-width: 0; min-height: 20px; padding: 0 6px; font-size: 12px; line-height: 1; color: var(--ga-text-secondary); background: transparent; border-color: transparent; }
      .rail button:hover { color: var(--ga-text-primary); background: var(--ga-control-hover); }
      .rail .close { display: none; }
      :host([sidebar-collapsed]) .sidebar { padding: 8px 2px; overflow: hidden; }
      :host([sidebar-collapsed]) .sidebar .content, :host([sidebar-collapsed]) .rail .spacer { display: none; }
      :host([sidebar-collapsed]) .rail { flex-direction: column; }
      .menu, .backdrop { display: none; }
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

      /* Phone width: the page takes the whole width and the sidebar is a drawer over it. Folding does
         not apply to a drawer, so the fold button gives way to a close button. */
      @media (max-width: ${DRAWER_MAX_PX}px) {
        .zones, :host([sidebar-side="left"]) .zones { grid-template-columns: minmax(0, 1fr); grid-template-areas: "main"; }
        main { padding: 10px 10px; }
        .menu { display: inline-flex; align-items: center; justify-content: center; min-width: 32px; min-height: 28px; padding: 0 6px; font-size: 16px; line-height: 1; }
        .sidebar, :host([sidebar-collapsed]) .sidebar {
          position: fixed;
          top: 0;
          bottom: 0;
          right: 0;
          z-index: 20;
          width: min(320px, calc(100vw - 48px));
          padding: 8px;
          overflow: auto;
          overscroll-behavior: contain;
          box-shadow: 0 0 24px rgb(0 0 0 / 0.45);
          transform: translateX(calc(100% + 24px));
          visibility: hidden;
          transition: transform 160ms ease, visibility 0s linear 160ms;
        }
        :host([sidebar-side="left"]) .sidebar { right: auto; left: 0; transform: translateX(calc(-100% - 24px)); }
        :host([drawer-open]) .sidebar { transform: none; visibility: visible; transition: transform 160ms ease; }
        :host([sidebar-collapsed]) .sidebar .content { display: grid; }
        :host([sidebar-collapsed]) .rail { flex-direction: row; }
        :host([sidebar-collapsed][sidebar-side="left"]) .rail { flex-direction: row-reverse; }
        :host([sidebar-collapsed]) .rail .spacer { display: block; }
        .rail .fold { display: none; }
        .rail .close { display: inline-block; }
        :host([drawer-open]) .backdrop { display: block; position: fixed; inset: 0; z-index: 19; background: rgb(0 0 0 / 0.4); }
      }
      @media (prefers-reduced-motion: reduce) {
        .sidebar, :host([drawer-open]) .sidebar { transition: none; }
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

    // The drawer is open for this page only: on a phone the sidebar starts closed.
    const drawer = signal(false);
    const menu = h("button", { class: "menu", type: "button", slot: "menu", "aria-controls": "sidebar", "aria-label": "Open the sidebar", title: "Devices, meter and Control Room", "on:click": () => (drawer.value = !drawer.peek()) }, "☰");
    const closeDrawer = (returnFocus: boolean) => {
      if (!drawer.peek()) return;
      drawer.value = false;
      if (returnFocus) menu.focus();
    };
    const fold = h("button", { class: "fold", type: "button", "data-testid": "sidebar-fold", "on:click": () => store.toggleSidebar() });
    const move = h("button", { class: "move", type: "button", "data-testid": "sidebar-move", "on:click": () => store.moveSidebar() });
    const close = h("button", { class: "close", type: "button", "aria-label": "Close the sidebar", "on:click": () => closeDrawer(true) }, "×");
    const sections: [SidebarSection, GaSection][] = [
      ["devices", h("ga-section", { heading: "Devices" }, h("ga-device-list"))],
      ["meter", h("ga-section", { heading: "Meter" }, h("ga-output-meters"))],
      ["controlRoom", h("ga-section", { heading: "Control Room" }, h("ga-control-room"))],
    ];
    for (const [id, section] of sections) {
      section.collapsed = store.sidebar.peek().sections[id] ?? false;
      section.addEventListener("toggle", () => store.setSidebarSection(id, section.collapsed));
    }
    const sidebar = h(
      "aside",
      { class: "zone sidebar", id: "sidebar", "aria-label": "Devices, meter and Control Room" },
      h("div", { class: "rail" }, move, h("span", { class: "spacer" }), fold, close),
      h("div", { class: "content" }, sections.map(([, section]) => section)),
    );
    const backdrop = h("div", { class: "backdrop", "aria-hidden": "true", "on:click": () => closeDrawer(false) });

    this.root.replaceChildren(
      h("ga-header", {}, menu),
      h("div", { class: "zones" }, main, sidebar, backdrop),
      // The mixer dock's zone.
      h("footer", { class: "zone lower", "aria-label": "Mixer" }, h("ga-section", { heading: "Mixer", collapsed: true }, h("p", { class: "placeholder" }, "Open the Mixer page for channel strips, faders and meters. A compact mixer here comes later."))),
      h("ga-notices"),
    );

    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && drawer.peek()) {
        event.preventDefault();
        closeDrawer(true);
      }
    };
    document.addEventListener("keydown", onKey);
    this.onDisconnect(() => document.removeEventListener("keydown", onKey));
    // Growing past phone width docks the sidebar again; coming back, the drawer starts closed.
    const phone = typeof matchMedia === "function" ? matchMedia(`(max-width: ${DRAWER_MAX_PX}px)`) : undefined;
    const onWidth = () => closeDrawer(false);
    phone?.addEventListener("change", onWidth);
    this.onDisconnect(() => phone?.removeEventListener("change", onWidth));

    this.watch(() => {
      const open = drawer.value;
      this.toggleAttribute("drawer-open", open);
      menu.setAttribute("aria-expanded", String(open));
      if (open) close.focus();
    });
    // A new address closes the drawer: picking a device card or a page is done with it.
    this.watch(() => {
      void route.value;
      untracked(() => closeDrawer(false));
    });
    this.watch(() => {
      const properties = cssProperties(store.theme.value);
      const style = document.documentElement.style;
      for (const [name, value] of Object.entries(properties)) style.setProperty(name, value);
    });
    this.watch(() => {
      const { side, collapsed } = store.sidebar.value;
      this.setAttribute("sidebar-side", side);
      this.toggleAttribute("sidebar-collapsed", collapsed);
      // The arrows point the way the sidebar will move.
      const outward = side === "right" ? "»" : "«";
      const inward = side === "right" ? "«" : "»";
      fold.textContent = collapsed ? inward : outward;
      fold.setAttribute("aria-label", collapsed ? "Expand the sidebar" : "Collapse the sidebar");
      fold.setAttribute("aria-expanded", String(!collapsed));
      const other = side === "right" ? "left" : "right";
      move.textContent = side === "right" ? "⇤" : "⇥";
      move.setAttribute("aria-label", `Move the sidebar to the ${other}`);
      move.title = `Move the sidebar to the ${other}`;
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
