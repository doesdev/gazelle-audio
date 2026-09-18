// <ga-header>: brand, page tabs, and the hardware-safety badges the spec requires to be always
// visible (§6.3): which backend is driving devices, dry-run, and the connection state. The theme
// picker sits at the end, then slot="menu", where the app puts its sidebar button for phones.
//
// Narrower than a laptop, the bar takes two lines: the brand and badges above, the page tabs and
// theme picker below, where the tabs scroll sideways within their line when they do not fit.

import { h } from "../core/dom.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href, PAGES, route } from "./router.ts";

const STATUS_TEXT = { open: "Connected", reconnecting: "Reconnecting…", closed: "Disconnected" } as const;

/** Each page tab's key for the explain mode. */
const PAGE_KEYS: Record<string, string> = {
  devices: "header.page.devices",
  workspace: "header.page.workspace",
  inputs: "header.page.inputs",
  outputs: "header.page.outputs",
  mixer: "header.page.mixer",
  routing: "header.page.routing",
  effects: "header.page.effects",
};

export class GaHeader extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; background: var(--ga-surface-panel); border-bottom: 1px solid var(--ga-surface-background); }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 0 12px; min-height: 40px; padding: 0 12px; }
      .brand { font-size: 20px; letter-spacing: 0.06em; }
      nav { display: flex; gap: 2px; }
      nav a { flex: none; white-space: nowrap; }
      nav a {
        padding: 4px 10px;
        border-radius: 3px;
        color: var(--ga-text-secondary);
        font-family: "Josefin Sans Variable", system-ui, sans-serif;
        font-size: 14px;
        font-weight: 600;
      }
      nav a:hover { background: var(--ga-control-hover); color: var(--ga-text-primary); }
      nav a[aria-current="page"] { background: var(--ga-control-active); color: var(--ga-text-primary); }
      .spacer { flex: 1; }
      .badge {
        padding: 2px 7px;
        border-radius: 3px;
        font-size: 10px;
        font-weight: 700;
        letter-spacing: 0.08em;
        text-transform: uppercase;
      }
      .backend { background: var(--ga-surface-inset); color: var(--ga-text-secondary); border: 1px solid var(--ga-border-subtle); }
      .backend[data-backend="usb"] { color: var(--ga-notice-warning); border-color: var(--ga-notice-warning); }
      .dry-run { background: var(--ga-state-dry-run); color: var(--ga-text-inverse); }
      .status { display: flex; align-items: center; gap: 6px; color: var(--ga-text-secondary); font-size: 11px; }
      .status::before { content: ""; width: 8px; height: 8px; border-radius: 50%; background: var(--ga-connection-closed); }
      .status[data-state="open"]::before { background: var(--ga-connection-open); }
      .status[data-state="reconnecting"]::before { background: var(--ga-connection-reconnecting); }
      @media (max-width: 959px) {
        .bar { gap: 2px 8px; padding: 4px 8px; }
        /* A zero-height line break, ordered between the two lines. */
        .bar::after { content: ""; order: 1; flex: 0 0 100%; }
        nav { order: 2; flex: 1 1 0; min-width: 0; overflow-x: auto; scrollbar-width: none; }
        .theme { order: 3; max-width: 120px; }
      }
      @media (max-width: 480px) {
        .brand { font-size: 17px; }
        nav a { padding: 4px 8px; }
        .theme { max-width: 96px; }
        /* The dot's colour carries the state; the words stay for screen readers and the tooltip. */
        .status-text { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
      }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const links = PAGES.map((page) => h("a", { href: href({ page: page.page }), "data-page": page.page, "data-explain": PAGE_KEYS[page.page] }, page.label));
    const backend = h("span", { class: "badge backend", "data-testid": "backend", "data-explain": "header.backend" });
    const dryRun = h("span", { class: "badge dry-run", "data-testid": "dry-run", "data-explain": "header.dry-run", title: "Commands report the bytes they would send; nothing is written to a device." }, "Dry run");
    const statusText = h("span", { class: "status-text" });
    const status = h("span", { class: "status", role: "status", "data-testid": "connection", "data-explain": "header.connection" }, statusText);
    const picker = h("select", { class: "theme", "aria-label": "Theme", "data-explain": "header.theme", "on:change": (event) => store.selectTheme((event.target as HTMLSelectElement).value) });

    this.root.replaceChildren(h("div", { class: "bar" }, h("span", { class: "brand title" }, "Gazelle"), h("nav", { "aria-label": "Pages" }, links), h("span", { class: "spacer" }), backend, dryRun, status, picker, h("slot", { name: "menu" })));

    this.watch(() => {
      // A surface is opened from the Workspace page, so that tab stays marked while one is shown.
      const current = route.value.page === "surface" ? "workspace" : route.value.page;
      for (const link of links) link.toggleAttribute("aria-current", link.dataset["page"] === current);
      for (const link of links) if (link.dataset["page"] === current) link.setAttribute("aria-current", "page");
    });
    this.watch(() => {
      const info = store.server.value;
      backend.textContent = info.backend || "unknown";
      backend.dataset["backend"] = info.backend;
      dryRun.hidden = !info.dry_run;
    });
    this.watch(() => {
      const state = store.status.value;
      status.dataset["state"] = state;
      statusText.textContent = STATUS_TEXT[state];
      status.title = STATUS_TEXT[state];
    });
    this.watch(() => {
      const { themes } = store.themeCatalog.value;
      const current = store.theme.value.id;
      picker.replaceChildren(...themes.map((theme) => h("option", { value: theme.id, selected: theme.id === current }, theme.name)));
      picker.value = current;
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-header": GaHeader;
  }
}
