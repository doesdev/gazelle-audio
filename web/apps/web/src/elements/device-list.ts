// <ga-device-list>: every connected device, by the name the user gave it, with a colour swatch
// from the theme's channel palette. Selecting one opens its status page; on the Devices page, the
// device shown is marked.

import { h } from "../core/dom.ts";
import { displayName } from "../store/store.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href, route } from "./router.ts";

export class GaDeviceList extends GaElement {
  static override styles = [
    sheet(`
      ul { list-style: none; margin: 0; padding: 0; }
      a {
        display: grid;
        grid-template-columns: 4px 1fr auto;
        align-items: center;
        gap: 8px;
        padding: 5px 8px 5px 0;
        border-radius: 3px;
      }
      a:hover { background: var(--ga-control-hover); }
      a[aria-current="page"] { background: var(--ga-control-active); }
      .swatch { align-self: stretch; border-radius: 2px; background: var(--swatch); }
      .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .family { color: var(--ga-text-muted); font-size: 10px; text-transform: uppercase; letter-spacing: 0.06em; }
      .empty { padding: 8px; color: var(--ga-text-muted); }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const list = h("ul", { "aria-label": "Devices" });
    this.root.replaceChildren(h("ga-section", { heading: "Devices" }, list));

    this.watch(() => {
      const devices = store.devices.value;
      const workspace = store.workspace.value;
      const current = route.value;
      const colours = Math.max(1, store.theme.value.palette.length);
      const selected = current.page === "devices" ? (current.id ?? store.deviceInView(false)) : undefined;
      list.replaceChildren(
        ...(devices.length === 0
          ? [h("li", { class: "empty" }, "No devices")]
          : devices.map((device, i) =>
              h(
                "li",
                {},
                h(
                  "a",
                  { href: href({ page: "devices", id: device.id }), "data-device-id": device.id, "aria-current": device.id === selected ? "page" : undefined },
                  h("span", { class: "swatch", style: `--swatch: var(--ga-channel-palette-${i % colours})` }),
                  h("span", { class: "name" }, displayName(device, workspace)),
                  h("span", { class: "family" }, device.family ?? "unknown"),
                ),
              ),
            )),
      );
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-device-list": GaDeviceList;
  }
}
