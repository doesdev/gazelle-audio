// <ga-workspace>: layout state shared by everyone using the server — device names and badge colours,
// cross-device surfaces, declared digital cables, and groups with their colour and collapsed state.
// Edits save automatically; controls are disabled while disconnected.
//
// Backup: Export downloads the workspace as a dated JSON file. Import reads a chosen file, checks
// its shape, says what it holds and asks before replacing; the server's own validation then
// decides, and its reason is shown if it refuses. The file's contents are passed on untouched.
// A workspace is layout only, so importing one sends nothing to a device.

import { h } from "../core/dom.ts";
import { effect, untracked } from "../core/signal.ts";
import { portName, portWidth } from "../store/cables.ts";
import { displayName, type DigitalPort, type Group } from "../store/store.ts";
import { describeChange, describeDiff, describeSection, describeSnapshot, formatWhen, SNAPSHOT_VERSION, type DeviceDiff, type SectionDiff } from "../store/snapshots.ts";
import { backupFileName, backupFileText, readWorkspaceFile, workspaceFileName, workspaceFileText, type WorkspaceSummary } from "../store/workspace-file.ts";
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
    count(summary.cables, "cable", "cables"),
    count(summary.controlRooms, "Control Room choice", "Control Room choices"),
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
      .cables { display: grid; gap: 4px; margin: 0; padding: 0; list-style: none; }
      .cable { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; padding: 4px 0; border-top: 1px solid var(--ga-border-subtle); }
      .cable:first-child { border-top: 0; }
      .cable-label { font-size: 12px; }
      .health { flex: 1; min-width: 0; font-size: 11px; }
      .warn { color: var(--ga-notice-warning, var(--ga-text-primary)); }
      .channels { width: 3.5em; }
      .diff-section { margin-top: 8px; }
      .diff-heading { margin: 0 0 2px; font-size: 11px; color: var(--ga-text-secondary); }
      .changes { display: grid; gap: 2px; margin: 0; padding: 0; list-style: none; }
      .change { display: flex; flex-wrap: wrap; gap: 4px 10px; font-size: 11px; }
      .change-label { flex: 1 1 220px; min-width: 0; }
      .change-value { flex: 0 0 auto; color: var(--ga-text-secondary); font-variant-numeric: tabular-nums; }
      .change.warn .change-value { color: var(--ga-notice-warning, var(--ga-text-primary)); }
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
      h("ga-section", { heading: "Digital cables" }, this.#cables()),
      h("ga-section", { heading: "Groups" }, groups),
      h("ga-section", { heading: "Snapshots" }, this.#snapshots()),
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

  /**
   * Digital cables: each declared cable with what is wrong along it (from both devices' reports,
   * followed while the page is open), removed behind a second click; and a form to declare one from
   * a device's digital output to another's input. A cable routes nothing (§4.5).
   */
  #cables(): HTMLElement {
    const store = useStore();
    const list = h("ul", { class: "cables", "data-testid": "cables" });
    const from = h("select", { "aria-label": "Cable from", "data-testid": "cable-from" });
    const to = h("select", { "aria-label": "Cable to", "data-testid": "cable-to" });
    const channels = h("input", { type: "number", min: 1, max: 8, value: 2, "aria-label": "Channels", class: "channels", "data-testid": "cable-channels" });
    const declare = h("button", { type: "button", "data-testid": "cable-declare" }, "Declare cable");
    const problem = h("p", { class: "problem", role: "alert", "data-testid": "cable-problem", hidden: true });

    // Each port of each attached device, by its ADAT ports' eights ("ADAT out 9–16") or whole.
    const ends = (side: "out" | "in") =>
      store.devices.value.flatMap((device) =>
        store.cables.portsOf(device.id, side).flatMap((port) => {
          const width = portWidth(port);
          const count = store.cables.portChannels(device.id, port);
          return Array.from({ length: Math.ceil(count / width) }, (_, k) => ({
            value: `${device.id}|${port}|${k * width}`,
            label: `${displayName(device, store.workspace.value)} ${portName(port)}${count <= width ? "" : ` ${k * width + 1}–${Math.min(count, (k + 1) * width)}`}`,
          }));
        }),
      );
    const parse = (value: string) => {
      const [device_id, port, first] = value.split("|");
      return { device_id: device_id ?? "", port: port as DigitalPort, first: Number(first) };
    };
    // The receiving ports follow the sending port's kind, and the channels its width.
    const matchTo = () => {
      const sending = parse(from.value).port;
      for (const option of to.options) option.hidden = option.value.split("|")[1] !== sending?.replace("_OUT", "_IN");
      if (to.selectedOptions[0]?.hidden !== false) to.value = [...to.options].find((o) => !o.hidden)?.value ?? "";
      channels.max = String(portWidth(sending ?? "SPDIF_OUT"));
      channels.value = channels.max;
    };
    from.addEventListener("change", matchTo);
    declare.addEventListener("click", () => {
      problem.hidden = true;
      try {
        store.cables.declare(parse(from.value), parse(to.value), Number(channels.value));
      } catch (error) {
        problem.textContent = error instanceof Error ? error.message : String(error);
        problem.hidden = false;
      }
    });

    this.watch(() => {
      const connected = store.connected.value;
      const outs = ends("out");
      const ins = ends("in");
      const [fromValue, toValue] = [from.value, to.value];
      from.replaceChildren(...outs.map((e) => h("option", { value: e.value }, e.label)));
      to.replaceChildren(...ins.map((e) => h("option", { value: e.value }, e.label)));
      if (outs.some((e) => e.value === fromValue)) from.value = fromValue;
      if (ins.some((e) => e.value === toValue)) to.value = toValue;
      untracked(matchTo);
      from.disabled = to.disabled = channels.disabled = declare.disabled = !connected || outs.length === 0 || store.workspace.value === undefined;
    });

    // Both ends' reports are followed while the page shows a cable, for its warnings.
    const watching = new Map<string, () => void>();
    const rows: (() => void)[] = [];
    let rendered = "";
    this.onDisconnect(() => {
      for (const off of watching.values()) off();
      for (const dispose of rows.splice(0)) dispose();
    });
    this.watch(() => {
      const cables = store.cables.list.value;
      const connected = store.connected.value;
      const attached = new Set(store.devices.value.filter((d) => d.family !== null).map((d) => d.id));
      const devices = [...new Set(cables.flatMap((c) => [c.from.device_id, c.to.device_id]))].filter((id) => attached.has(id));
      untracked(() => {
        for (const [id, off] of watching) {
          if (!devices.includes(id)) {
            off();
            watching.delete(id);
          }
        }
        for (const id of devices) if (!watching.has(id)) watching.set(id, store.watchReport(id, "0x73"));
      });
      // Rows are rebuilt when the cables change; each row's warning follows the reports on its own, so a
      // Remove waiting for its confirming click is not rebuilt away by every report.
      const key = JSON.stringify([cables, connected, cables.map((c) => store.cables.label(c))]);
      if (key === rendered) return;
      rendered = key;
      untracked(() => {
        for (const dispose of rows.splice(0)) dispose();
        if (cables.length === 0) {
          list.replaceChildren(h("li", { class: "placeholder" }, "No cables declared. Say which digital output is plugged into which input, and surfaces can say where a signal comes from and warn when the clocks disagree."));
          return;
        }
        list.replaceChildren(
          ...cables.map((cable) => {
            const health = h("span", { class: "health", "data-testid": `cable-health-${cable.id}` });
            rows.push(
              effect(() => {
                const problems = store.cables.health(cable);
                health.classList.toggle("warn", problems.length > 0);
                health.classList.toggle("muted", problems.length === 0);
                health.textContent = problems.length === 0 ? "Nothing wrong reported" : `⚠ ${problems.join(" ")}`;
              }),
            );
            let armed: ReturnType<typeof setTimeout> | undefined;
            const remove = h("button", {
              type: "button",
              "data-testid": `cable-remove-${cable.id}`,
              title: "Remove this cable (click twice); nothing changes on the devices",
              disabled: !connected,
              "on:click": () => {
                if (armed !== undefined) {
                  clearTimeout(armed);
                  store.cables.remove(cable.id);
                  return;
                }
                remove.textContent = "Confirm";
                armed = setTimeout(() => {
                  armed = undefined;
                  remove.textContent = "Remove";
                }, 3000);
              },
            }, "Remove");
            return h(
              "li",
              { class: "cable", "data-testid": `cable-row-${cable.id}` },
              h("span", { class: "cable-label" }, store.cables.label(cable)),
              health,
              remove,
            );
          }),
        );
      });
    });

    return h(
      "div",
      { class: "backup" },
      list,
      h("div", { class: "actions" }, from, h("span", { "aria-hidden": "true" }, "→"), to, h("label", { class: "note" }, "Channels ", channels), declare),
      problem,
      h("p", { class: "note" }, "A cable only says what is plugged in. It routes nothing and changes no clock; set those on each device."),
    );
  }

  /**
   * Snapshots: take one with a name, list them with when and from which devices, rename, delete,
   * and compare one with the devices as they are now.
   *
   * Everything here reads. Taking a snapshot asks every attached device for its state and changes
   * nothing on one; comparing reads them again. Putting a snapshot back is not built (spec §2.3):
   * it waits for a session at the hardware, and until then the diff is what a snapshot is for.
   */
  #snapshots(): HTMLElement {
    const store = useStore();
    const snapshots = store.snapshots;
    const list = h("div", { class: "surfaces", "data-testid": "snapshots" });
    const name = h("input", { type: "text", placeholder: "Snapshot name", "aria-label": "New snapshot name", "data-testid": "snapshot-new-name" });
    const take = h("button", { type: "button", "data-testid": "snapshot-take" }, "Take snapshot");
    const problem = h("p", { class: "problem", role: "alert", "data-testid": "snapshot-problem", hidden: true });
    const diff = h("div", { "data-testid": "snapshot-diff" });

    void snapshots.loadOnce();

    const takeOne = () => {
      if (name.value.trim() === "") {
        name.focus();
        return;
      }
      const asked = name.value;
      void snapshots.take(asked).then((taken) => {
        if (taken !== undefined && name.value === asked) name.value = "";
      });
    };
    take.addEventListener("click", takeOne);
    name.addEventListener("keydown", (event) => {
      if (event.key === "Enter") takeOne();
    });

    this.watch(() => {
      const reason = snapshots.problem.value;
      problem.textContent = reason ?? "";
      problem.hidden = reason === undefined;
    });

    let rendered = "";
    this.watch(() => {
      const listed = snapshots.list.value;
      const busy = snapshots.busy.value;
      const connected = store.connected.value;
      const known = snapshots.known.value;
      take.disabled = name.disabled = !connected || busy !== undefined;
      take.textContent = busy === "taking" ? "Reading the devices…" : "Take snapshot";
      const key = JSON.stringify([listed, connected, busy]);
      if (key === rendered) return;
      rendered = key;
      if (listed.length === 0) {
        list.replaceChildren(h("p", { class: "placeholder" }, known ? "No snapshots yet. A snapshot records the workspace and every attached device's settings, so you can see what has changed since." : "Loading…"));
        return;
      }
      list.replaceChildren(
        ...listed.map((summary) => {
          const field = h("input", { type: "text", class: "surface-name", value: summary.name, "aria-label": `Name of ${summary.name}`, "data-testid": `snapshot-rename-${summary.id}`, disabled: !connected });
          commitOnEnter(field, (value) => void snapshots.rename(summary.id, value), () => snapshots.list.peek().find((s) => s.id === summary.id)?.name ?? summary.name);
          const compare = h("button", { type: "button", class: "open", "data-testid": `snapshot-compare-${summary.id}`, title: "Read every device again and show what differs; nothing is sent", disabled: !connected || busy !== undefined, "on:click": () => void snapshots.compare(summary.id) }, busy === "comparing" ? "Reading…" : "Compare with now");
          let armed: ReturnType<typeof setTimeout> | undefined;
          const remove = h("button", {
            type: "button",
            "data-testid": `snapshot-delete-${summary.id}`,
            title: "Delete this snapshot (click twice); nothing changes on the devices",
            disabled: !connected,
            "on:click": () => {
              if (armed !== undefined) {
                clearTimeout(armed);
                void snapshots.remove(summary.id);
                return;
              }
              remove.textContent = "Confirm";
              armed = setTimeout(() => {
                armed = undefined;
                remove.textContent = "Delete";
              }, 3000);
            },
          }, "Delete");
          return h(
            "div",
            { class: "surface", "data-testid": `snapshot-row-${summary.id}` },
            field,
            h("span", { class: "muted summary" }, `${formatWhen(summary.created)} · ${describeSnapshot(summary)}`),
            compare,
            remove,
          );
        }),
      );
    });

    this.watch(() => {
      const shown = snapshots.diff.value;
      if (shown === undefined) {
        diff.replaceChildren();
        return;
      }
      diff.replaceChildren(
        h(
          "div",
          { class: "confirm", role: "group", "aria-label": `What differs from ${shown.snapshot.name}` },
          h("p", { "data-testid": "snapshot-diff-summary" }, `${shown.snapshot.name}, taken ${formatWhen(shown.snapshot.created)}, against the devices now: ${describeDiff(shown)}`),
          ...shown.devices.map((device) => this.#deviceDiff(device)),
          shown.workspace.length === 0 ? null : this.#sectionDiff("workspace-workspace", { section: "workspace", title: "Workspace", changes: shown.workspace }),
          h("div", { class: "actions" }, h("button", { type: "button", "data-testid": "snapshot-diff-close", "on:click": () => snapshots.closeDiff() }, "Close")),
        ),
      );
    });

    return h(
      "div",
      { class: "backup" },
      list,
      h("div", { class: "actions" }, name, take),
      problem,
      diff,
      h("p", { class: "note" }, "A snapshot records the workspace and, for each attached device, its mixer, routing, input settings, outputs, clock and device settings, read fresh. Taking one and comparing it only read: nothing is sent to a device, and putting a snapshot back is not built yet."),
    );
  }

  /** One device's part of a comparison: its sections, or why it has none. */
  #deviceDiff(device: DeviceDiff): HTMLElement {
    const heading = h("p", { class: "note", "data-testid": `snapshot-diff-device-${device.device_id}` }, device.missing
      ? `${device.model || device.device_id} is in the snapshot but is not attached now, so none of it can be compared.`
      : device.added
        ? `${device.model || device.device_id} is attached now but was not when the snapshot was taken.`
        : `${device.model || device.device_id}: ${device.changes === 0 ? "nothing differs" : describeSection({ section: "", title: "", changes: device.sections.flatMap((s) => s.changes) })}`);
    return h("div", {}, heading, ...device.sections.map((section) => this.#sectionDiff(`${device.device_id}-${section.section}`, section)));
  }

  /** One section of a comparison: its changes, each as a line a person can read. */
  #sectionDiff(key: string, section: SectionDiff): HTMLElement {
    return h(
      "div",
      { class: "diff-section", "data-testid": `snapshot-diff-${key}` },
      h("p", { class: "diff-heading" }, `${section.title} — ${describeSection(section)}`),
      h(
        "ul",
        { class: "changes" },
        section.changes.map((change) =>
          h("li", { class: change.kind === "unknown" ? "change warn" : "change" }, h("span", { class: "change-label" }, change.label), h("span", { class: "change-value" }, describeChange(change))),
        ),
      ),
    );
  }

  /** Export and import. The chosen file and its confirmation live only as long as the page. */
  #backup(): HTMLElement {
    const store = useStore();
    const exportButton = h("button", { type: "button", "data-testid": "workspace-export" }, "Export");
    const exportAll = h("button", { type: "button", "data-testid": "workspace-export-backup" }, "Export with snapshots");
    const file = h("input", { type: "file", class: "file", accept: ".json,application/json", "data-testid": "workspace-import-file", "aria-label": "Workspace file to import" });
    const importButton = h("button", { type: "button", "data-testid": "workspace-import" }, "Import…");
    const outcome = h("div");
    let busy = false;

    const show = (...children: HTMLElement[]) => outcome.replaceChildren(...children);
    const problem = (text: string) => show(h("p", { class: "problem", role: "alert", "data-testid": "workspace-import-problem" }, text));

    const download = (text: string, filename: string) => {
      const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
      h("a", { href: url, download: filename }).click();
      setTimeout(() => URL.revokeObjectURL(url), 0);
    };

    exportButton.addEventListener("click", () => {
      const workspace = store.workspace.peek();
      if (workspace === undefined) return;
      download(workspaceFileText(workspace), workspaceFileName(new Date()));
    });

    // A full backup (spec §3.3): the workspace plus every snapshot whole, in one file that imports
    // back as both. Fetching the snapshots' values is why it is a second button rather than the
    // only one — an export of names and groups should not wait on a megabyte of captured state.
    exportAll.addEventListener("click", async () => {
      const workspace = store.workspace.peek();
      if (workspace === undefined) return;
      exportAll.disabled = true;
      const snapshots = await store.snapshots.all();
      exportAll.disabled = false;
      if (snapshots === undefined) {
        problem(`The backup was not saved. ${store.snapshots.problem.peek() ?? ""}`.trim());
        return;
      }
      download(backupFileText(workspace, snapshots), backupFileName(new Date()));
    });

    importButton.addEventListener("click", () => file.click());
    file.addEventListener("change", async () => {
      const chosen = file.files?.[0];
      // Cleared at once, so choosing the same file again (after fixing it, say) is a change too.
      file.value = "";
      if (chosen === undefined) return;
      const read = readWorkspaceFile(await chosen.text(), store.workspace.peek()?.version ?? 1, SNAPSHOT_VERSION);
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
        // Snapshots are added after the workspace, and only if it went in: a backup half applied
        // should leave the snapshots out rather than beside a workspace that is not theirs. They
        // are add-only, so one already here keeps the moment it recorded (spec §3.3).
        const added = refused !== undefined || read.snapshots.length === 0 ? undefined : await store.snapshots.importAll(read.snapshots);
        busy = false;
        if (refused !== undefined) {
          problem(`${chosen.name} was not imported. ${refused}`);
          return;
        }
        const snapshotNote = added === undefined ? "" : ` ${added.added.length} ${added.added.length === 1 ? "snapshot was" : "snapshots were"} added${added.skipped.length === 0 ? "" : `; ${added.skipped.length} already here ${added.skipped.length === 1 ? "was" : "were"} kept as ${added.skipped.length === 1 ? "it is" : "they are"}`}.`;
        show(h("p", { class: "done", role: "status", "data-testid": "workspace-import-status" }, `Imported ${chosen.name}.${snapshotNote}`));
      });
      cancel.addEventListener("click", () => show());
      show(
        h(
          "div",
          { class: "confirm", role: "group", "aria-label": "Confirm import", "data-testid": "workspace-import-confirm" },
          h("p", {}, `Replace this workspace with ${chosen.name}? It holds ${describeSummary(read.summary)}${read.snapshots.length === 0 ? "" : `, and ${read.snapshots.length} ${read.snapshots.length === 1 ? "snapshot" : "snapshots"}`}.`),
          read.snapshots.length === 0 ? null : h("p", { class: "note" }, "Snapshots are added, not replaced: any already here keep the moment they recorded. Nothing in them is sent to a device."),
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
      exportAll.disabled = !connected || !loaded || store.snapshots.busy.value !== undefined;
      importButton.disabled = file.disabled = !connected || !loaded;
    });

    return h(
      "div",
      { class: "backup" },
      h("p", { class: "note" }, "Export saves the device names, groups, links, mixer layouts and saved layouts to a file; Export with snapshots saves those and every snapshot's values too. Import reads either back, replacing the workspace and adding snapshots that are not already here. Neither touches the devices: levels, routing and input settings stay as they are."),
      h("div", { class: "actions" }, exportButton, exportAll, importButton, file),
      outcome,
    );
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-workspace": GaWorkspace;
  }
}
