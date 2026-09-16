// <ga-workspace>: layout state shared by everyone using the server — device names, and groups with
// their colour and collapsed state. Edits save automatically; controls are disabled while
// disconnected.

import { h } from "../core/dom.ts";
import type { Group } from "../store/store.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";

export class GaWorkspace extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; max-width: 720px; }
      ga-section + ga-section { margin-top: 10px; }
      .saving { font-size: 11px; color: var(--ga-text-muted); }
      table { width: 100%; border-collapse: collapse; }
      th { text-align: left; font-weight: 500; font-size: 11px; color: var(--ga-text-secondary); padding: 4px 10px; }
      td { padding: 4px 10px; border-top: 1px solid var(--ga-border-subtle); }
      td input { width: 100%; }
      ul { list-style: none; margin: 0; padding: 0 0 0 10px; }
      .group { display: flex; align-items: center; gap: 8px; padding: 4px 0; }
      .chip { width: 14px; height: 14px; min-height: 14px; padding: 0; border-radius: 3px; border: 1px solid var(--ga-border-strong); }
      .chip[aria-pressed="true"] { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .none { background: var(--ga-surface-inset); }
      .group-name { min-width: 120px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const saving = h("span", { class: "saving", slot: "actions", role: "status" });
    const names = h("tbody");
    const groups = h("div");
    this.root.replaceChildren(
      h("ga-section", { heading: "Device names" }, saving, h("table", {}, h("thead", {}, h("tr", {}, h("th", {}, "Device"), h("th", {}, "Name"))), names)),
      h("ga-section", { heading: "Groups" }, groups),
    );

    this.watch(() => {
      saving.textContent = store.saving.value ? "Saving…" : "";
    });

    this.watch(() => {
      const devices = store.devices.value;
      const workspace = store.workspace.value;
      const connected = store.connected.value;
      names.replaceChildren(
        ...devices.map((device) => {
          const input = h("input", {
            value: workspace?.aliases[device.id] ?? "",
            placeholder: device.model ?? device.id,
            "aria-label": `Name for ${device.id}`,
            disabled: !connected,
          });
          commitOnEnter(input, (value) => store.renameDevice(device.id, value), () => store.workspace.peek()?.aliases[device.id] ?? "", store.view<string | undefined>(`draft:workspace:${device.id}:name`, undefined));
          return h("tr", {}, h("td", {}, h("span", { class: "readout" }, device.id)), h("td", {}, input));
        }),
      );
    });

    this.watch(() => {
      const workspace = store.workspace.value;
      const connected = store.connected.value;
      const palette = store.theme.value.palette;
      if (workspace === undefined) {
        groups.replaceChildren(h("p", { class: "placeholder" }, "The workspace has not loaded."));
        return;
      }
      if (workspace.groups.length === 0) {
        groups.replaceChildren(h("p", { class: "placeholder" }, "No groups yet."));
        return;
      }
      const render = (items: readonly Group[]): HTMLUListElement =>
        h(
          "ul",
          {},
          items.map((group) =>
            h(
              "li",
              {},
              h(
                "div",
                { class: "group" },
                h("button", { type: "button", disabled: !connected, "aria-expanded": String(!group.collapsed), "on:click": () => store.toggleGroup(group.id) }, group.collapsed ? "▸" : "▾"),
                h("span", { class: "group-name", style: group.color ? `border-left: 4px solid ${group.color}; padding-left: 6px;` : "" }, group.name),
                h("button", { type: "button", class: "chip none", title: "No colour", "aria-label": `No colour for ${group.name}`, "aria-pressed": String(group.color === undefined), disabled: !connected, "on:click": () => store.setGroupColor(group.id, undefined) }),
                palette.map((colour, i) =>
                  h("button", {
                    type: "button",
                    class: "chip",
                    style: `background: ${colour}`,
                    title: colour,
                    "aria-label": `Colour ${i + 1} for ${group.name}`,
                    "aria-pressed": String(group.color === colour),
                    disabled: !connected,
                    "on:click": () => store.setGroupColor(group.id, colour),
                  }),
                ),
              ),
              group.collapsed || group.children.length === 0 ? null : render(group.children),
            ),
          ),
        );
      groups.replaceChildren(render(workspace.groups));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-workspace": GaWorkspace;
  }
}
