// <ga-workspace>: layout state shared by everyone using the server — device names and badge colours,
// cross-device surfaces, and groups with their colour and collapsed state. Edits save automatically;
// controls are disabled while disconnected.
//
// Backup: Export downloads the workspace as a dated JSON file. Import reads a chosen file, checks
// its shape, says what it holds and asks before replacing; the server's own validation then
// decides, and its reason is shown if it refuses. The file's contents are passed on untouched.
// A workspace is layout only, so importing one sends nothing to a device.

import { h } from "../core/dom.ts";
import { displayName, type Group } from "../store/store.ts";
import { readWorkspaceFile, workspaceFileName, workspaceFileText, type WorkspaceSummary } from "../store/workspace-file.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";
import { href } from "./router.ts";

/** "2 device names, 1 group and 1 link": the parts a file holds, the empty ones left out. */
export function describeSummary(summary: WorkspaceSummary): string {
  const count = (n: number, one: string, many: string) => (n === 0 ? undefined : `${n} ${n === 1 ? one : many}`);
  const parts = [
    count(summary.names, "device name", "device names"),
    count(summary.groups, "group", "groups"),
    count(summary.links, "link", "links"),
    count(summary.mixers, "mixer layout", "mixer layouts"),
    count(summary.layouts, "saved layout", "saved layouts"),
    count(summary.surfaces, "surface", "surfaces"),
  ].filter((part) => part !== undefined);
  if (parts.length === 0) return "nothing: an empty workspace";
  return parts.length === 1 ? parts[0]! : `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}`;
}

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
      .backup { padding: 8px 10px; display: grid; gap: 8px; }
      .note { margin: 0; font-size: 11px; color: var(--ga-text-muted); }
      .actions { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .file { display: none; }
      .confirm { display: grid; gap: 8px; padding: 8px 10px; border: 1px solid var(--ga-border-strong); border-radius: 4px; background: var(--ga-surface-inset); }
      .confirm p { margin: 0; }
      .problem { margin: 0; color: var(--ga-notice-error, var(--ga-text-primary)); }
      .done { margin: 0; color: var(--ga-text-secondary); }
      .colour-cell { display: flex; align-items: center; gap: 6px; }
      .colour { width: 32px; min-width: 32px; height: 22px; padding: 0 2px; }
      .surfaces { display: grid; gap: 4px; }
      .surface { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; padding: 4px 0; border-top: 1px solid var(--ga-border-subtle); }
      .surface:first-child { border-top: 0; }
      .surface-name { flex: 0 1 200px; min-width: 120px; }
      .summary { flex: 1; min-width: 0; font-size: 11px; }
      .open { color: var(--ga-accent); }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const saving = h("span", { class: "saving", slot: "actions", role: "status" });
    const names = h("tbody");
    const groups = h("div");
    this.root.replaceChildren(
      h("ga-section", { heading: "Device names" }, saving, h("table", {}, h("thead", {}, h("tr", {}, h("th", {}, "Device"), h("th", {}, "Name"), h("th", {}, "Colour"))), names)),
      h("ga-section", { heading: "Surfaces" }, this.#surfaces()),
      h("ga-section", { heading: "Groups" }, groups),
      h("ga-section", { heading: "Backup" }, this.#backup()),
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
          // The badge colour a surface's strips carry for this device (Q15): chosen, or the palette's until then.
          const chosen = workspace?.device_colors?.[device.id];
          const colour = h("input", {
            type: "color",
            class: "colour",
            value: store.surfaces.deviceColor(device.id) ?? "#808080",
            "aria-label": `Colour for ${device.id}`,
            "data-testid": `device-colour-${device.id}`,
            disabled: !connected,
            // On change, not input: the row is rebuilt as the workspace changes, which would close the picker.
            "on:change": () => store.surfaces.setDeviceColor(device.id, colour.value),
          });
          const clear = h("button", { type: "button", class: "clear", "aria-label": `Clear the colour for ${device.id}`, title: "Back to the theme's colour", "data-testid": `device-colour-clear-${device.id}`, disabled: !connected || chosen === undefined, "on:click": () => store.surfaces.setDeviceColor(device.id, undefined) }, "Clear");
          return h("tr", {}, h("td", {}, h("span", { class: "readout" }, device.id)), h("td", {}, input), h("td", { class: "colour-cell" }, colour, clear));
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

  /**
   * Cross-device mix surfaces: each with how many strips and which devices it shows, a link to open
   * it, its name to edit in place, and Delete behind a second click (the devices keep everything; a
   * surface is only what to show). A name and "+ New surface" make one.
   */
  #surfaces(): HTMLElement {
    const store = useStore();
    const list = h("div", { class: "surfaces", "data-testid": "surfaces" });
    const name = h("input", { type: "text", placeholder: "Surface name", "aria-label": "New surface name", "data-testid": "surface-new-name" });
    const create = h("button", { type: "button", "data-testid": "surface-create" }, "+ New surface");
    const make = () => {
      if (name.value.trim() === "") {
        name.focus();
        return;
      }
      if (store.surfaces.create(name.value) !== undefined) name.value = "";
    };
    create.addEventListener("click", make);
    name.addEventListener("keydown", (event) => {
      if (event.key === "Enter") make();
    });

    let rendered = "";
    this.watch(() => {
      const surfaces = store.surfaces.list.value;
      const connected = store.connected.value;
      const devices = store.devices.value;
      const workspace = store.workspace.value;
      create.disabled = name.disabled = !connected || workspace === undefined;
      const rows = surfaces.map((surface) => ({
        id: surface.id,
        name: surface.name,
        strips: surface.strips.length,
        devices: store.surfaces.devicesOf(surface.id).map((id) => {
          const device = devices.find((d) => d.id === id);
          return device === undefined ? `${id} (not connected)` : displayName(device, workspace);
        }),
      }));
      const key = JSON.stringify([rows, connected]);
      if (key === rendered) return;
      rendered = key;
      if (rows.length === 0) {
        list.replaceChildren(h("p", { class: "placeholder" }, "No surfaces yet. A surface puts strips from any device side by side: drum preamps on one interface next to the cue mix on another."));
        return;
      }
      list.replaceChildren(
        ...rows.map((row) => {
          const field = h("input", { type: "text", class: "surface-name", value: row.name, "aria-label": `Name of ${row.name}`, "data-testid": `surface-rename-${row.id}`, disabled: !connected });
          commitOnEnter(field, (value) => {
            try {
              store.surfaces.rename(row.id, value);
            } catch {
              field.value = store.surfaces.surface(row.id)?.name ?? row.name;
            }
          }, () => store.surfaces.list.peek().find((s) => s.id === row.id)?.name ?? "");
          let armed: ReturnType<typeof setTimeout> | undefined;
          const remove = h("button", {
            type: "button",
            "data-testid": `surface-delete-${row.id}`,
            title: "Delete this surface (click twice); nothing changes on the devices",
            disabled: !connected,
            "on:click": () => {
              if (armed !== undefined) {
                clearTimeout(armed);
                store.surfaces.remove(row.id);
                return;
              }
              remove.textContent = "Confirm";
              armed = setTimeout(() => {
                armed = undefined;
                remove.textContent = "Delete";
              }, 3000);
            },
          }, "Delete");
          const summary = `${row.strips} ${row.strips === 1 ? "strip" : "strips"}${row.devices.length === 0 ? "" : ` · ${row.devices.join(", ")}`}`;
          return h(
            "div",
            { class: "surface", "data-testid": `surface-row-${row.id}` },
            field,
            h("span", { class: "muted summary" }, summary),
            h("a", { class: "open", href: href({ page: "surface", id: row.id }), "data-testid": `surface-open-${row.id}` }, "Open"),
            remove,
          );
        }),
      );
    });
    return h("div", { class: "backup" }, list, h("div", { class: "actions" }, name, create));
  }

  /** Export and import. The chosen file and its confirmation live only as long as the page. */
  #backup(): HTMLElement {
    const store = useStore();
    const exportButton = h("button", { type: "button", "data-testid": "workspace-export" }, "Export");
    const file = h("input", { type: "file", class: "file", accept: ".json,application/json", "data-testid": "workspace-import-file", "aria-label": "Workspace file to import" });
    const importButton = h("button", { type: "button", "data-testid": "workspace-import" }, "Import…");
    const outcome = h("div");
    let busy = false;

    const show = (...children: HTMLElement[]) => outcome.replaceChildren(...children);
    const problem = (text: string) => show(h("p", { class: "problem", role: "alert", "data-testid": "workspace-import-problem" }, text));

    exportButton.addEventListener("click", () => {
      const workspace = store.workspace.peek();
      if (workspace === undefined) return;
      const url = URL.createObjectURL(new Blob([workspaceFileText(workspace)], { type: "application/json" }));
      h("a", { href: url, download: workspaceFileName(new Date()) }).click();
      setTimeout(() => URL.revokeObjectURL(url), 0);
    });

    importButton.addEventListener("click", () => file.click());
    file.addEventListener("change", async () => {
      const chosen = file.files?.[0];
      // Cleared at once, so choosing the same file again (after fixing it, say) is a change too.
      file.value = "";
      if (chosen === undefined) return;
      const read = readWorkspaceFile(await chosen.text(), store.workspace.peek()?.version ?? 1);
      if (!read.ok) {
        problem(`${chosen.name} was not imported: it ${read.problem}.`);
        return;
      }
      const connectedIds = new Set(store.devices.peek().map((device) => device.id));
      const absent = read.summary.devices.filter((id) => !connectedIds.has(id));
      const replace = h("button", { type: "button", "data-testid": "workspace-import-replace" }, "Replace workspace");
      const cancel = h("button", { type: "button", "data-testid": "workspace-import-cancel" }, "Cancel");
      replace.addEventListener("click", async () => {
        if (busy) return;
        busy = true;
        replace.disabled = cancel.disabled = true;
        const refused = await store.replaceWorkspace(read.workspace);
        busy = false;
        if (refused === undefined) show(h("p", { class: "done", role: "status", "data-testid": "workspace-import-status" }, `Imported ${chosen.name}.`));
        else problem(`${chosen.name} was not imported. ${refused}`);
      });
      cancel.addEventListener("click", () => show());
      show(
        h(
          "div",
          { class: "confirm", role: "group", "aria-label": "Confirm import", "data-testid": "workspace-import-confirm" },
          h("p", {}, `Replace this workspace with ${chosen.name}? It holds ${describeSummary(read.summary)}.`),
          absent.length === 0 ? null : h("p", { class: "note" }, `It also names devices that are not connected: ${absent.join(", ")}. Their names and layouts apply when a device with that id is attached.`),
          h("p", { class: "note" }, "Everything in the current workspace is replaced, for everyone using this server. Export first to keep a copy."),
          h("div", { class: "actions" }, replace, cancel),
        ),
      );
    });

    this.watch(() => {
      const connected = store.connected.value;
      const loaded = store.workspace.value !== undefined;
      exportButton.disabled = !connected || !loaded;
      importButton.disabled = file.disabled = !connected || !loaded;
    });

    return h(
      "div",
      { class: "backup" },
      h("p", { class: "note" }, "Export saves the device names, groups, links, mixer layouts and saved layouts to a file. Import replaces them from one. Neither touches the devices: levels, routing and input settings stay as they are."),
      h("div", { class: "actions" }, exportButton, importButton, file),
      outcome,
    );
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-workspace": GaWorkspace;
  }
}
