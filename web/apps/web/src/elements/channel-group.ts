// <ga-channel-group device-id="…" group-id="…">: a run of channels in one group, which the mixer page
// places inside it. A band over the channels holds the group's name, colour, collapse and remove
// (removing keeps the channels, ungrouped). The band sits in the row's top padding, so grouped and
// ungrouped faders stay level; collapsed, the group is a narrow tile showing its name.

import { h } from "../core/dom.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";

/** The band's height; the mixer row reserves this much above every channel. */
export const GROUP_BAND_PX = 22;

export class GaChannelGroup extends GaElement {
  static override styles = [
    sheet(`
      :host { position: relative; display: flex; flex-direction: column; min-width: 0; }
      .band {
        position: absolute;
        top: -${GROUP_BAND_PX + 2}px;
        left: 0;
        right: 0;
        display: flex;
        align-items: center;
        gap: 2px;
        height: ${GROUP_BAND_PX}px;
        padding: 0 3px;
        border-radius: 3px 3px 0 0;
        background: var(--group-colour, var(--ga-section-header));
        color: var(--ga-text-inverse);
      }
      .band input[type="text"] {
        flex: 1;
        min-width: 0;
        min-height: 16px;
        padding: 0 3px;
        border: 0;
        background: rgb(0 0 0 / 0.18);
        color: inherit;
        font: 600 11px "Josefin Sans Variable", system-ui, sans-serif;
      }
      .band input[type="color"] { flex: 0 0 18px; width: 18px; min-height: 16px; padding: 0; border: 0; background: none; cursor: pointer; }
      .band button { min-width: 0; min-height: 16px; padding: 0 4px; border: 0; background: rgb(0 0 0 / 0.18); color: inherit; font-size: 10px; }
      .members { display: flex; flex: 1; gap: 2px; min-height: 0; }
      .vertical { display: none; font: 600 11px "Josefin Sans Variable", system-ui, sans-serif; white-space: nowrap; overflow: hidden; writing-mode: vertical-rl; }
      :host([collapsed]) { margin-top: -${GROUP_BAND_PX + 2}px; }
      :host([collapsed]) .members, :host([collapsed]) .band input, :host([collapsed]) .band .remove { display: none; }
      :host([collapsed]) .band { position: static; flex: 1; flex-direction: column; height: auto; padding: 4px 2px; border-radius: 3px; }
      :host([collapsed]) .vertical { display: block; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const groupId = this.getAttribute("group-id") ?? "";
    const channels = store.channels(deviceId);
    const current = () => channels.layout.peek().groups.find((g) => g.id === groupId);

    const toggle = h("button", { type: "button", "data-explain": "group.toggle", "on:click": () => channels.toggleGroup(groupId) });
    const name = h("input", { type: "text", "aria-label": "Group name", "data-explain": "group.name" });
    const showName = commitOnEnter(name, (value) => channels.renameGroup(groupId, value.trim() === "" ? (current()?.name ?? "") : value.trim()), () => current()?.name ?? "", store.view<string | undefined>(`draft:mixer:${deviceId}:group:${groupId}:name`, undefined));
    const color = h("input", { type: "color", "data-explain": "group.colour", "on:input": () => channels.setGroupColor(groupId, color.value) });
    const remove = h("button", { type: "button", class: "remove", "data-explain": "group.remove", "on:click": () => channels.removeGroup(groupId) }, "×");
    const vertical = h("span", { class: "vertical" });

    this.root.replaceChildren(h("div", { class: "band" }, toggle, name, color, remove, vertical), h("div", { class: "members" }, h("slot")));

    this.watch(() => {
      const group = channels.layout.value.groups.find((g) => g.id === groupId);
      if (group === undefined) return;
      showName(group.name);
      color.value = group.color ?? "#5a5f66";
      color.setAttribute("aria-label", `${group.name} colour`);
      remove.setAttribute("aria-label", `Remove group ${group.name}`);
      remove.title = "Remove the group; its channels stay";
      vertical.textContent = group.name;
      this.toggleAttribute("collapsed", group.collapsed);
      toggle.textContent = group.collapsed ? "▸" : "▾";
      toggle.setAttribute("aria-label", `${group.collapsed ? "Expand" : "Collapse"} group ${group.name}`);
      toggle.setAttribute("aria-expanded", String(!group.collapsed));
      if (group.color === undefined) this.style.removeProperty("--group-colour");
      else this.style.setProperty("--group-colour", group.color);
    });
    this.watch(() => {
      const connected = store.connected.value;
      for (const control of [toggle, name, color, remove]) control.disabled = !connected;
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-channel-group": GaChannelGroup;
  }
}
