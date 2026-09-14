// <ga-header>: brand, page tabs, and the hardware-safety badges the spec requires to be always
// visible (§6.3): which backend is driving devices, dry-run, and the connection state. The theme
// picker sits at the end.

import { h } from "../core/dom.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href, PAGES, route } from "./router.ts";

const STATUS_TEXT = { open: "Connected", reconnecting: "Reconnecting…", closed: "Disconnected" } as const;

export class GaHeader extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; background: var(--ga-surface-panel); border-bottom: 1px solid var(--ga-surface-background); }
      .bar { display: flex; align-items: center; gap: 12px; min-height: 40px; padding: 0 12px; }
      .brand { font-size: 20px; letter-spacing: 0.06em; }
      nav { display: flex; gap: 2px; }
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
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const links = PAGES.map((page) => h("a", { href: href({ page: page.page }), "data-page": page.page }, page.label));
    const backend = h("span", { class: "badge backend", "data-testid": "backend" });
    const dryRun = h("span", { class: "badge dry-run", "data-testid": "dry-run", title: "Commands report the bytes they would send; nothing is written to a device." }, "Dry run");
    const status = h("span", { class: "status", role: "status", "data-testid": "connection" });
    const picker = h("select", { class: "theme", "aria-label": "Theme", "on:change": (event) => store.selectTheme((event.target as HTMLSelectElement).value) });

    this.root.replaceChildren(h("div", { class: "bar" }, h("span", { class: "brand title" }, "Gazelle"), h("nav", { "aria-label": "Pages" }, links), h("span", { class: "spacer" }), backend, dryRun, status, picker));

    this.watch(() => {
      const current = route.value.page;
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
      status.textContent = STATUS_TEXT[state];
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
