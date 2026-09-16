// <ga-routing device-id="…">: the device's whole routing, laid out as the Studio+ panel's routing
// tab does: every destination group is a row of channel cells showing their source, and the sources
// sit above as chips. Select source channels (shift-click extends a run) and click a cell, or drag
// the chips onto a cell: the run fills that cell and the ones after it, cut at the row's end, with
// one read and one write per row. Delete mutes a focused cell; "Mute row" mutes a row. Mixer inputs
// belong to the Mixer page's channels, so their rows are shown but not edited here.

import { h } from "../core/dom.ts";
import type { RouteSlot } from "../store/routing.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href } from "./router.ts";

/** How far a pointer moves before a press on a chip is a drag. */
const DRAG_PX = 4;

export class GaRouting extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 10px; }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .spacer { flex: 1; }
      .note { margin: 0; font-size: 11px; color: var(--ga-text-muted); }
      /* The bytes can be long (set_routing is 128 hex digits): one line, cut with an ellipsis, all of it in the tooltip. */
      .last-sent { display: flex; min-width: 0; max-width: 100%; font-size: 11px; white-space: nowrap; }
      .last-sent code { min-width: 0; overflow: hidden; text-overflow: ellipsis; font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      h2 { margin: 0 0 6px; }
      .table { display: grid; gap: 3px; overflow-x: auto; padding-bottom: 4px; }
      /* Row tools sit by the name, so wide rows keep them in view. */
      .row { display: grid; grid-template-columns: 124px 74px max-content; align-items: center; gap: 6px; }
      .row > .spacer-tools { display: block; }
      .label { display: flex; align-items: center; gap: 6px; min-width: 0; font-size: 11px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .swatch { flex: 0 0 8px; height: 14px; border-radius: 2px; background: var(--group-colour); }
      .cells { display: flex; gap: 2px; }
      .chip, .cell { flex: 0 0 56px; width: 56px; min-width: 0; min-height: 22px; padding: 0 3px; border: 1px solid var(--ga-border-subtle); border-radius: 3px; font-size: 9px; text-align: left; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .chip { border-left: 3px solid var(--group-colour); background: var(--ga-control-background); cursor: grab; touch-action: none; user-select: none; }
      .chip[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .cell { background: var(--ga-surface-inset); color: var(--ga-text-muted); }
      .cell[data-routed] { border-left: 3px solid var(--source-colour, var(--ga-accent)); background: var(--ga-surface-raised); color: var(--ga-text-primary); }
      .cell[data-drop] { outline: 2px solid var(--ga-accent); outline-offset: -2px; }
      .cell[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .tools button { min-height: 22px; padding: 0 6px; font-size: 10px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const topology = store.topology(deviceId);
    if (topology === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so its routing is not known.`));
      return;
    }
    const routing = store.routing(deviceId);
    void routing.loadAll();

    const shortLabel = (group: number, channel: number) => {
      const source = topology.inputs[group];
      if (source === undefined) return `${group}:${channel + 1}`;
      const base = source.name.replace(/ (PLAY|IN|OUT)$/, "");
      if (source.channels <= 1) return base;
      // "USB 1·3" rather than "USB 1 3" when the name already ends in a number.
      return `${base}${/\d$/.test(base) ? "·" : " "}${channel + 1}`;
    };

    const devices = h("select", {
      "aria-label": "Device",
      "on:change": (event) => {
        location.hash = href({ page: "routing", id: (event.target as HTMLSelectElement).value });
      },
    });
    const reload = h("button", { type: "button", "on:click": () => void routing.loadAll() }, "Read from device");
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent" });

    // The selection: a run of channels in one source group. It is kept per device for the tab.
    const kept = store.view<{ group: number; anchor: number; from: number; to: number } | undefined>(`routing:${deviceId}:selection`, undefined);
    let selection = kept.peek();
    const chips: HTMLButtonElement[][] = [];
    const selected = (): RouteSlot[] => (selection === undefined ? [] : Array.from({ length: selection.to - selection.from + 1 }, (_, i) => ({ source: (selection as { group: number }).group, channel: (selection as { from: number }).from + i })));
    const paint = () =>
      chips.forEach((row, group) => row.forEach((chip, channel) => chip.setAttribute("aria-pressed", String(selection !== undefined && selection.group === group && channel >= selection.from && channel <= selection.to))));
    const select = (group: number, channel: number, extend: boolean) => {
      selection = extend && selection?.group === group ? { ...selection, from: Math.min(selection.anchor, channel), to: Math.max(selection.anchor, channel) } : { group, anchor: channel, from: channel, to: channel };
      kept.value = selection;
      paint();
    };
    const readOnly = (destination: number) => topology.outputs[destination]?.type === "MIXER_IN";
    const fill = (destination: number, channel: number, sources: RouteSlot[]) => {
      const group = topology.outputs[destination];
      if (group === undefined || readOnly(destination) || sources.length === 0) return;
      void routing.routeMany(destination, sources.slice(0, group.channels - channel).map((source, i) => ({ channel: channel + i, source })));
    };

    let suppressClick = false;
    const sourceRows = topology.inputs.flatMap((group, g) => {
      if (group.type === "MUTE") return [];
      chips[g] = Array.from({ length: group.channels }, (_, c) =>
        h(
          "button",
          {
            type: "button",
            class: "chip",
            "data-source": `${g}:${c}`,
            "data-testid": `source-${g}-${c}`,
            title: `${group.name} ${c + 1}`,
            "on:click": (event) => {
              if (suppressClick) {
                suppressClick = false;
                return;
              }
              select(g, c, (event as MouseEvent).shiftKey);
            },
          },
          shortLabel(g, c),
        ),
      );
      return [h("div", { class: "row", style: `--group-colour: ${group.color}` }, h("span", { class: "label" }, h("span", { class: "swatch" }), group.name), h("span", { class: "spacer-tools" }), h("div", { class: "cells" }, chips[g]))];
    });

    paint();

    const destinationRows = topology.outputs.map((group, d) => {
      const locked = readOnly(d);
      const cells = Array.from({ length: group.channels }, (_, c) =>
        h("button", {
          type: "button",
          class: "cell",
          "data-destination": `${d}:${c}`,
          "data-testid": `dest-${d}-${c}`,
          "data-readonly": locked,
          "aria-label": `${group.name} ${c + 1}`,
          "aria-disabled": locked ? "true" : undefined,
          "on:click": () => {
            if (!locked && selection !== undefined) fill(d, c, selected());
          },
          "on:keydown": (event) => {
            const key = (event as KeyboardEvent).key;
            if (!locked && (key === "Delete" || key === "Backspace")) void routing.route(d, c, null);
          },
        }),
      );
      const mute = h("button", { type: "button", "data-testid": `mute-row-${d}`, "data-readonly": locked, disabled: locked, title: locked ? "Mixer inputs are set by the Mixer page's channels" : `Mute every ${group.name} channel`, "on:click": () => void routing.routeMany(d, cells.map((_, c) => ({ channel: c, source: null }))) }, "Mute row");
      this.watch(() => {
        const slots = routing.destination(d).value;
        cells.forEach((cell, c) => {
          const slot = slots?.[c];
          const routed = slot !== undefined && slot.source !== routing.mute;
          cell.toggleAttribute("data-routed", routed);
          cell.textContent = slot === undefined ? "?" : routed ? shortLabel(slot.source, slot.channel) : "—";
          cell.title = slot === undefined ? `${group.name} ${c + 1}: not read from the device` : routed ? `${group.name} ${c + 1} ← ${topology.inputs[slot.source]?.name ?? "?"} ${slot.channel + 1}` : `${group.name} ${c + 1}: muted`;
          const colour = routed ? topology.inputs[slot.source]?.color : undefined;
          if (colour === undefined) cell.style.removeProperty("--source-colour");
          else cell.style.setProperty("--source-colour", colour);
        });
      });
      return h("div", { class: "row" }, h("span", { class: "label", title: locked ? "Mixer inputs are set by the Mixer page's channels" : group.name }, group.name), h("span", { class: "tools" }, mute), h("div", { class: "cells" }, cells));
    });

    const sources = h("section", {}, h("h2", {}, "Sources"), h("div", { class: "table" }, sourceRows));
    const destinations = h("section", {}, h("h2", {}, "Destinations"), h("div", { class: "table" }, destinationRows));
    this.root.replaceChildren(
      h("div", { class: "bar" }, devices, reload, h("span", { class: "spacer" }), lastSent),
      h("p", { class: "note" }, "Pick sources (shift-click for a run) and click a destination cell, or drag them onto one; a run fills that cell and those after it. Delete mutes a cell. Mixer inputs are set on the Mixer page."),
      sources,
      destinations,
    );

    // Dragging chips onto a cell.
    let drag: { pointer: number; x: number; y: number; moved: boolean; over: HTMLElement | undefined } | undefined;
    const cellAt = (x: number, y: number) => (this.root.elementFromPoint(x, y) as HTMLElement | null)?.closest<HTMLElement>("[data-destination]") ?? undefined;
    sources.addEventListener("pointerdown", (event) => {
      const chip = (event.target as HTMLElement).closest<HTMLElement>("[data-source]");
      if (chip === null || event.button !== 0 || !store.connected.peek()) return;
      const [g, c] = (chip.dataset["source"] ?? "").split(":").map(Number) as [number, number];
      if (!(selection !== undefined && selection.group === g && c >= selection.from && c <= selection.to)) select(g, c, event.shiftKey);
      drag = { pointer: event.pointerId, x: event.clientX, y: event.clientY, moved: false, over: undefined };
      chip.setPointerCapture(event.pointerId);
    });
    sources.addEventListener("pointermove", (event) => {
      if (drag?.pointer !== event.pointerId) return;
      drag.moved ||= Math.hypot(event.clientX - drag.x, event.clientY - drag.y) > DRAG_PX;
      if (!drag.moved) return;
      const over = cellAt(event.clientX, event.clientY);
      if (over === drag.over) return;
      drag.over?.removeAttribute("data-drop");
      drag.over = over !== undefined && over.dataset["readonly"] === undefined ? over : undefined;
      drag.over?.setAttribute("data-drop", "");
    });
    const endDrag = (event: PointerEvent, drop: boolean) => {
      if (drag?.pointer !== event.pointerId) return;
      const { moved, over } = drag;
      drag = undefined;
      over?.removeAttribute("data-drop");
      suppressClick = moved;
      if (drop && moved && over !== undefined) {
        const [d, c] = (over.dataset["destination"] ?? "").split(":").map(Number) as [number, number];
        fill(d, c, selected());
      }
    };
    sources.addEventListener("pointerup", (event) => endDrag(event, true));
    sources.addEventListener("pointercancel", (event) => endDrag(event, false));

    this.watch(() => {
      const known = store.devices.value.filter((d) => d.family !== null);
      devices.replaceChildren(...known.map((d) => h("option", { value: d.id, selected: d.id === deviceId }, d.model ?? d.id)));
      devices.value = deviceId;
    });
    this.watch(() => {
      const connected = store.connected.value;
      devices.disabled = !connected;
      // Mixer-input rows stay disabled: the Mixer page's channels set them.
      for (const button of this.root.querySelectorAll<HTMLButtonElement>("button")) button.disabled = !connected || button.hasAttribute("data-readonly");
    });
    this.watch(() => {
      const sent = store.lastSent.value;
      const dryRun = store.server.value.dry_run;
      if (sent === undefined || sent.deviceId !== deviceId) {
        lastSent.textContent = dryRun ? "Dry run: nothing is written to the device" : "";
        return;
      }
      lastSent.replaceChildren(`${sent.dryRun ? "Dry run, would send" : "Sent"} ${sent.command}: `, h("code", { title: sent.hex }, sent.hex));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-routing": GaRouting;
  }
}
