// <ga-surface surface-id="…">: a cross-device mix surface, at #/surface/<id>.
// A bar holds one mix menu per device whose channels or masters are on it (the user's choice:
// one selected mix per device, which a strip may pin instead), each device's clock, and a picker to
// add a strip: a device, a kind, and an item, or both ends of a declared cable at once (the sender's
// port and the receiver's inputs). Each cable between devices on the surface has a line saying what
// is wrong along it, if anything (clock, lock, signal). Below, the strips in a row that scrolls sideways,
// each with its device's badge (`ga-surface-strip`) and small tools: a grip to drag it elsewhere
// (as the Mixer page's channels move), ‹ › for the keyboard, a mix pin for channels and masters, and
// × to take it off the surface, which asks for a second click. Taking a strip off changes nothing on
// a device; every control on a strip sends what the device's own page would.

import { h } from "../core/dom.ts";
import { untracked } from "../core/signal.ts";
import { meterDeflection } from "../store/mixer.ts";
import { channelSpan, portName, portWidth } from "../store/cables.ts";
import { displayName, SAMPLE_RATES, type Cable, type NewStrip, type SurfaceStrip } from "../store/store.ts";
import { meterGradient } from "../themes/theme.ts";
import { GaElement, LAST_SENT_STYLES, sheet, showLastSent, useStore } from "./element.ts";
import { href } from "./router.ts";
import { keepScroll } from "./view-state.ts";

/** How long a first click on × waits for its confirmation. */
const ARM_MS = 3000;

/** What the picker adds: a strip of a kind, or both ends of a cable. */
type PickerKind = SurfaceStrip["kind"] | "cable";
const KIND_LABELS: Record<PickerKind, string> = { channel: "Mixer channel", master: "Mix master", input: "Input", output: "Output", port: "Digital out", cable: "Both ends of a cable", label: "Label" };
const INPUT_LABELS = { preamp: "Preamp", line: "Line", adat: "ADAT", spdif: "S/PDIF" } as const;

export class GaSurface extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 8px; flex: 1; min-height: 0; }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 12px; }
      .bar select, .bar input, .bar button { min-height: 26px; }
      .name { margin: 0; font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 16px; font-weight: 600; }
      .spacer { flex: 1; }
      .caption { font-size: 11px; color: var(--ga-text-secondary); }
      .device-mix { display: flex; align-items: center; gap: 4px; }
      .dot { width: 10px; height: 10px; border-radius: 2px; background: var(--device-colour, var(--ga-border-strong)); }
      .clocks { display: flex; flex-wrap: wrap; gap: 4px 12px; font-size: 11px; color: var(--ga-text-secondary); }
      .clock { display: flex; align-items: center; gap: 4px; }
      .health { display: grid; gap: 2px; margin: 0; padding: 0; list-style: none; font-size: 11px; }
      .health:empty { display: none; }
      .health li { color: var(--ga-text-secondary); }
      .health .warn { color: var(--ga-notice-warning, var(--ga-text-primary)); }
      .add { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
      ${LAST_SENT_STYLES}
      .strips {
        position: relative;
        display: flex;
        flex: 1;
        gap: 4px;
        min-height: 360px;
        padding: 4px;
        overflow-x: auto;
        overflow-y: hidden;
        border-radius: 3px;
        background: var(--ga-surface-inset);
      }
      .slot { display: flex; flex-direction: column; flex: 0 0 96px; gap: 2px; min-width: 0; min-height: 0; }
      .slot[data-kind="input"], .slot[data-kind="output"], .slot[data-kind="port"] { flex-basis: 156px; }
      .slot[data-kind="label"] { flex-basis: 56px; }
      .slot[data-dragging] { opacity: 0.5; }
      .slot ga-surface-strip { flex: 1 1 auto; }
      .tools { display: flex; gap: 1px; }
      .tools button, .tools select { min-width: 0; min-height: 18px; height: 18px; padding: 0 2px; font-size: 10px; line-height: 1; }
      .tools select { flex: 1; width: 0; }
      .tools .grip { cursor: grab; touch-action: none; }
      .tools .remove[data-armed] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .drop { position: absolute; top: 4px; bottom: 4px; z-index: 2; width: 2px; background: var(--ga-accent); pointer-events: none; }
      .empty { align-self: center; margin: 0 auto; }
      .content { display: contents; }
      .content[hidden] { display: none; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const surfaceId = this.getAttribute("surface-id") ?? "";
    const surfaces = store.surfaces;

    const title = h("h2", { class: "name", "data-testid": "surface-name", "data-explain": "surface.name" });
    const mixes = h("div", { class: "bar", "aria-label": "Mixes" });
    const clocks = h("div", { class: "clocks", "data-testid": "surface-clocks" });
    const health = h("ul", { class: "health", "data-testid": "surface-cables" });
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent", "data-explain": "page.last-sent" });
    const strips = h("div", { class: "strips", "data-testid": "surface-strips" });
    const indicator = h("div", { class: "drop", hidden: true });
    const missing = h("p", { class: "placeholder", hidden: true }, "This surface does not exist; it may have been deleted. ", h("a", { href: href({ page: "workspace" }), "data-explain": "surface.to-workspace" }, "Surfaces are on the Workspace page."));
    const content = h("div", { class: "content" }, h("div", { class: "bar" }, title, mixes, h("span", { class: "spacer" }), lastSent), clocks, health, this.#picker(surfaceId), strips);
    this.root.replaceChildren(missing, content);

    this.watch(() => {
      const surface = surfaces.surface(surfaceId);
      missing.hidden = surface !== undefined || store.workspace.value === undefined;
      content.hidden = surface === undefined;
      title.textContent = surface?.name ?? "";
    });

    // One mix menu for each device with channels or masters on the surface.
    let mixesKey = "";
    this.watch(() => {
      const surface = surfaces.surface(surfaceId);
      const devices = surfaces.devicesOf(surfaceId).filter((id) => surface?.strips.some((s) => s.device_id === id && (s.kind === "channel" || s.kind === "master")) && store.topology(id) !== undefined);
      const shown = devices.map((id) => {
        const entry = store.devices.value.find((d) => d.id === id);
        const channels = store.channels(id);
        return { id, name: entry === undefined ? id : displayName(entry, store.workspace.value), colour: surfaces.deviceColor(id), mix: surfaces.mixOf(surfaceId, id), names: Array.from({ length: channels.mixCount }, (_, m) => channels.mixName(m)) };
      });
      const key = JSON.stringify([shown, store.connected.value]);
      if (key === mixesKey) return;
      mixesKey = key;
      mixes.replaceChildren(
        ...shown.map((d) => {
          const select = h(
            "select",
            { "aria-label": `${d.name} mix`, "data-testid": `surface-mix-${d.id}`, "data-explain": "surface.mix", "data-explain-name": d.name, disabled: !store.connected.peek(), "on:change": () => surfaces.setMix(surfaceId, d.id, Number(select.value)) },
            d.names.map((name, m) => h("option", { value: String(m) }, name)),
          );
          select.value = String(d.mix);
          return h("label", { class: "device-mix", style: d.colour === undefined ? "" : `--device-colour: ${d.colour}` }, h("span", { class: "dot", "aria-hidden": "true" }), h("span", { class: "caption" }, d.name), select);
        }),
      );
    });

    // Each device's clock, followed while the surface is open: two devices joined by a cable must agree.
    const watching = new Map<string, () => void>();
    this.onDisconnect(() => {
      for (const off of watching.values()) off();
    });
    this.watch(() => {
      const devices = surfaces.devicesOf(surfaceId).filter((id) => store.topology(id) !== undefined && store.devices.value.some((d) => d.id === id));
      untracked(() => {
        for (const [id, off] of watching) {
          if (!devices.includes(id)) {
            off();
            watching.delete(id);
          }
        }
        for (const id of devices) if (!watching.has(id)) watching.set(id, store.watchReport(id, "0x73"));
      });
      clocks.replaceChildren(
        ...devices.map((id) => {
          const entry = store.devices.value.find((d) => d.id === id);
          const clock = store.clockState(id);
          const card = store.deviceCard(id);
          const sources = store.clock(id)?.sources ?? [];
          const text = clock === undefined || card?.reporting !== true ? "clock not reported yet" : `${SAMPLE_RATES[clock.rate] ?? "?"}, ${sources[clock.source] ?? "?"}, ${clock.locked ? "locked" : "not locked"}`;
          const colour = surfaces.deviceColor(id);
          return h("span", { class: "clock", "data-testid": `clock-${id}`, "data-explain": "surface.clock", style: colour === undefined ? "" : `--device-colour: ${colour}` }, h("span", { class: "dot", "aria-hidden": "true" }), `${entry === undefined ? id : displayName(entry, store.workspace.value)}: ${text}`);
        }),
      );
    });

    // Each cable between two devices on the surface, and what is wrong along it (display only).
    this.watch(() => {
      const devices = surfaces.devicesOf(surfaceId);
      const cables = store.cables.list.value.filter((c) => devices.includes(c.from.device_id) && devices.includes(c.to.device_id));
      health.replaceChildren(
        ...cables.map((cable) => {
          const problems = store.cables.health(cable);
          return h(
            "li",
            { "data-testid": `cable-health-${cable.id}`, "data-explain": "surface.cable-health" },
            store.cables.label(cable),
            ": ",
            problems.length === 0 ? h("span", {}, "nothing wrong reported") : h("span", { class: "warn", role: "status" }, `⚠ ${problems.join(" ")}`),
          );
        }),
      );
    });

    // The strips, rebuilt when the strips on the surface or their order change.
    let structure: string | undefined;
    this.watch(() => {
      const surface = surfaces.surface(surfaceId);
      const connected = store.connected.value;
      const key = JSON.stringify([surface?.strips.map((s) => [s.id, s.kind, s.device_id, s.mix]), connected]);
      if (key === structure) return;
      structure = key;
      untracked(() => {
        const list = surface?.strips ?? [];
        if (list.length === 0) {
          strips.replaceChildren(h("p", { class: "placeholder empty" }, "No strips yet. Add one above: a mixer channel, a mix master, an input or an output, from any device."), indicator);
          return;
        }
        strips.replaceChildren(...list.map((strip, index) => this.#slot(surfaceId, strip, index, list.length, connected)), indicator);
      });
    });
    this.onDisconnect(keepScroll(strips, store.view(`surface:${surfaceId}:scroll`, 0), "left"));

    // Drag to move, as on the Mixer page: the grip starts it, and the drop's place among the other strips is the new index.
    let dragging: { id: string; pointer: number; element: HTMLElement } | undefined;
    const dropAt = (x: number, id: string) => {
      const others = [...strips.querySelectorAll<HTMLElement>(".slot")].filter((e) => e.dataset["stripId"] !== id);
      const index = others.findIndex((e) => {
        const r = e.getBoundingClientRect();
        return x < r.left + r.width / 2;
      });
      return { index: index < 0 ? others.length : index, others };
    };
    strips.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || !store.connected.peek()) return;
      const grip = event.composedPath().find((n): n is HTMLElement => n instanceof HTMLElement && n.hasAttribute("data-grip"));
      const element = grip?.closest<HTMLElement>(".slot");
      const id = element?.dataset["stripId"];
      if (element === null || element === undefined || id === undefined) return;
      dragging = { id, pointer: event.pointerId, element };
      strips.setPointerCapture(event.pointerId);
      element.setAttribute("data-dragging", "");
      event.preventDefault();
    });
    strips.addEventListener("pointermove", (event) => {
      if (dragging?.pointer !== event.pointerId) return;
      const { index, others } = dropAt(event.clientX, dragging.id);
      const row = strips.getBoundingClientRect();
      const target = others[index];
      const edge = target !== undefined ? target.getBoundingClientRect().left - 2 : (others.at(-1)?.getBoundingClientRect().right ?? row.left) + 1;
      indicator.style.left = `${edge - row.left + strips.scrollLeft}px`;
      indicator.hidden = false;
    });
    const finish = (event: PointerEvent, drop: boolean) => {
      if (dragging?.pointer !== event.pointerId) return;
      const { id, element } = dragging;
      dragging = undefined;
      element.removeAttribute("data-dragging");
      indicator.hidden = true;
      if (drop) surfaces.moveStrip(surfaceId, id, dropAt(event.clientX, id).index);
    };
    strips.addEventListener("pointerup", (event) => finish(event, true));
    strips.addEventListener("pointercancel", (event) => finish(event, false));

    this.watch(() => {
      const theme = store.theme.value;
      // Meter gradient stops are dBFS; place them on the scale the meters use, as the Mixer page does.
      this.style.setProperty("--mixer-meter-gradient", meterGradient(theme.meter.gradient, undefined, "to top", (db) => meterDeflection(-db)));
      this.style.setProperty("--surface-meter-gradient", meterGradient(theme.meter.gradient, undefined, "to right", (db) => meterDeflection(-db)));
    });
    this.watch(() => {
      showLastSent(lastSent, store);
      const sent = store.lastSent.value;
      const onSurface = sent !== undefined && surfaces.devicesOf(surfaceId).includes(sent.deviceId);
      if (sent === undefined || !onSurface) {
        lastSent.textContent = store.server.value.dry_run ? "Dry run: nothing is written to the devices" : "";
        return;
      }
      const entry = store.devices.peek().find((d) => d.id === sent.deviceId);
      lastSent.replaceChildren(`${sent.dryRun ? "Dry run, would send" : "Sent"} ${sent.command} to ${entry === undefined ? sent.deviceId : displayName(entry, store.workspace.peek())}: `, h("code", { title: sent.hex }, sent.hex));
    });
  }

  /** A strip with its tools above it. */
  #slot(surfaceId: string, strip: SurfaceStrip, index: number, count: number, connected: boolean): HTMLElement {
    const store = useStore();
    const surfaces = store.surfaces;
    const move = (to: number) => surfaces.moveStrip(surfaceId, strip.id, to);
    const tools: HTMLElement[] = [
      h("button", { type: "button", class: "grip", "data-grip": "", title: "Drag to move", "aria-hidden": "true", tabindex: -1, disabled: !connected, "data-explain": "surface.grip" }, "⠿"),
      h("button", { type: "button", "aria-label": "Move left", "data-testid": `strip-left-${strip.id}`, "data-explain": "surface.move-left", disabled: !connected || index === 0, "on:click": () => move(index - 1) }, "‹"),
      h("button", { type: "button", "aria-label": "Move right", "data-testid": `strip-right-${strip.id}`, "data-explain": "surface.move-right", disabled: !connected || index === count - 1, "on:click": () => move(index + 1) }, "›"),
    ];
    const deviceId = strip.device_id;
    if ((strip.kind === "channel" || strip.kind === "master") && deviceId !== undefined && store.topology(deviceId) !== undefined) {
      const channels = store.channels(deviceId);
      const pin = h(
        "select",
        {
          "aria-label": "Mix this strip shows",
          title: "Follow the surface's mix for this device, or keep to one mix",
          "data-testid": `strip-pin-${strip.id}`,
          "data-explain": "surface.pin",
          "data-no-wheel": true,
          disabled: !connected,
          "on:change": () => surfaces.pin(surfaceId, strip.id, pin.value === "" ? undefined : Number(pin.value)),
        },
        h("option", { value: "" }, "Follow"),
        Array.from({ length: channels.mixCount }, (_, m) => h("option", { value: String(m) }, `Mix ${m + 1}`)),
      );
      pin.value = strip.mix === undefined ? "" : String(strip.mix);
      tools.push(pin);
    } else {
      tools.push(h("span", { style: "flex: 1" }));
    }
    let armed: ReturnType<typeof setTimeout> | undefined;
    const remove = h("button", {
      type: "button",
      class: "remove",
      "aria-label": "Take this strip off the surface",
      title: "Take off the surface (click twice); nothing changes on the device",
      "data-testid": `strip-remove-${strip.id}`,
      "data-explain": "surface.remove",
      disabled: !connected,
      "on:click": () => {
        if (armed !== undefined) {
          clearTimeout(armed);
          surfaces.removeStrip(surfaceId, strip.id);
          return;
        }
        remove.setAttribute("data-armed", "");
        armed = setTimeout(() => {
          armed = undefined;
          remove.removeAttribute("data-armed");
        }, ARM_MS);
      },
    }, "×");
    tools.push(remove);
    return h(
      "div",
      { class: "slot", "data-strip-id": strip.id, "data-kind": strip.kind, "data-testid": `surface-strip-${strip.id}` },
      h("div", { class: "tools" }, tools),
      h("ga-surface-strip", { "surface-id": surfaceId, "strip-id": strip.id }),
    );
  }

  /**
   * Both ends of a cable, side by side: the sender's port strip (the ADAT port holding the cable's
   * first channel) and one input strip per channel it brings into the receiver.
   */
  #addCableEnds(surfaceId: string, cable: Cable): void {
    const store = useStore();
    const width = portWidth(cable.from.port);
    const strips: NewStrip[] = [{ kind: "port", device_id: cable.from.device_id, port: cable.from.port as "SPDIF_OUT" | "ADAT_OUT", first: Math.floor(cable.from.first / width) * width }];
    const kind = cable.to.port === "ADAT_IN" ? "adat" : "spdif";
    for (let i = 0; i < cable.channels; i++) strips.push({ kind: "input", device_id: cable.to.device_id, input: { kind, channel: cable.to.first + i } });
    try {
      for (const strip of strips) store.surfaces.addStrip(surfaceId, strip);
    } catch (error) {
      store.reportError(error instanceof Error ? error.message : String(error));
    }
  }

  /** "+ Strip": a device, a kind, and an item of that kind on that device. */
  #picker(surfaceId: string): HTMLElement {
    const store = useStore();
    const device = h("select", { "aria-label": "Strip device", "data-testid": "strip-device", "data-explain": "surface.add-device" });
    const kind = h("select", { "aria-label": "Strip kind", "data-testid": "strip-kind", "data-explain": "surface.add-kind" }, (Object.keys(KIND_LABELS) as PickerKind[]).map((k) => h("option", { value: k }, KIND_LABELS[k])));
    const item = h("select", { "aria-label": "Strip item", "data-testid": "strip-item", "data-explain": "surface.add-item" });
    const text = h("input", { type: "text", placeholder: "Label text", "aria-label": "Label text", "data-testid": "strip-text", "data-explain": "surface.add-text" });
    const add = h("button", { type: "button", "data-testid": "strip-add", "data-explain": "surface.add" }, "+ Strip");

    const fill = () => {
      const deviceId = device.value;
      const chosen = kind.value as PickerKind;
      text.hidden = chosen !== "label";
      item.hidden = chosen === "label";
      device.disabled = chosen === "label" || chosen === "cable" || !store.connected.peek();
      const options: HTMLOptionElement[] = [];
      if (chosen === "cable") {
        for (const cable of store.cables.list.peek()) options.push(h("option", { value: cable.id }, store.cables.label(cable)));
      } else if (store.topology(deviceId) !== undefined) {
        if (chosen === "channel") {
          const channels = store.channels(deviceId);
          for (const c of channels.layout.peek().channels) options.push(h("option", { value: c.id }, channels.displayName(c)));
        } else if (chosen === "master") {
          const channels = store.channels(deviceId);
          options.push(h("option", { value: "" }, "The surface's mix"));
          for (let m = 0; m < channels.mixCount; m++) options.push(h("option", { value: String(m) }, `${channels.mixName(m)} (pinned)`));
        } else if (chosen === "input") {
          const inputs = store.inputs(deviceId);
          for (let i = 0; i < inputs.preampCount; i++) options.push(h("option", { value: `preamp:${i}` }, `${INPUT_LABELS.preamp} ${i + 1}`));
          for (const group of inputs.digital) for (let i = 0; i < group.count; i++) options.push(h("option", { value: `${group.kind}:${i}` }, `${INPUT_LABELS[group.kind]} ${i + 1}`));
        } else if (chosen === "output") {
          for (const output of store.outputs(deviceId).outputs) options.push(h("option", { value: String(output.id) }, output.name));
        } else if (chosen === "port") {
          for (const port of store.cables.portsOf(deviceId, "out") as ("SPDIF_OUT" | "ADAT_OUT")[]) {
            const width = portWidth(port);
            const channels = store.cables.portChannels(deviceId, port);
            for (let first = 0; first + width <= channels; first += width) options.push(h("option", { value: `${port}:${first}` }, channels <= width ? portName(port) : `${portName(port)} ${channelSpan(first + 1, first + width)}`));
          }
        }
      }
      const empty = chosen === "channel" ? "No channels: set some up on the Mixer page" : chosen === "cable" ? "No cables: declare one on the Workspace page" : "Nothing to add";
      item.replaceChildren(...(options.length > 0 ? options : [h("option", { value: "", disabled: true }, empty)]));
      add.disabled = !store.connected.peek() || (chosen !== "label" && (options.length === 0 || (chosen !== "cable" && store.topology(deviceId) === undefined)));
    };
    device.addEventListener("change", fill);
    kind.addEventListener("change", fill);

    add.addEventListener("click", () => {
      const chosen = kind.value as PickerKind;
      if (chosen === "cable") {
        const cable = store.cables.list.peek().find((c) => c.id === item.value);
        if (cable !== undefined) this.#addCableEnds(surfaceId, cable);
        return;
      }
      let strip: NewStrip;
      if (chosen === "label") strip = { kind: "label", text: text.value.trim() };
      else if (chosen === "channel") strip = { kind: "channel", device_id: device.value, channel: item.value };
      else if (chosen === "master") strip = item.value === "" ? { kind: "master", device_id: device.value } : { kind: "master", device_id: device.value, mix: Number(item.value) };
      else if (chosen === "output") strip = { kind: "output", device_id: device.value, output: Number(item.value) };
      else if (chosen === "port") {
        const [port, first] = item.value.split(":");
        strip = { kind: "port", device_id: device.value, port: port as "SPDIF_OUT", first: Number(first) };
      }
      else {
        const [input, channel] = item.value.split(":");
        strip = { kind: "input", device_id: device.value, input: { kind: input as "preamp", channel: Number(channel) } };
      }
      try {
        if (store.surfaces.addStrip(surfaceId, strip) !== undefined && chosen === "label") text.value = "";
      } catch (error) {
        store.reportError(error instanceof Error ? error.message : String(error));
      }
    });

    this.watch(() => {
      const known = store.devices.value.filter((d) => d.family !== null);
      const workspace = store.workspace.value;
      void store.connected.value;
      const current = device.value;
      device.replaceChildren(...known.map((d) => h("option", { value: d.id }, displayName(d, workspace))));
      if (known.some((d) => d.id === current)) device.value = current;
      // The channels a device has change as its layout does, and the cables as they are declared.
      for (const d of known) void store.channels(d.id).layout.value;
      void store.cables.list.value;
      untracked(fill);
    });

    return h("div", { class: "add", role: "group", "aria-label": "Add a strip" }, h("span", { class: "caption" }, "Add"), device, kind, item, text, add);
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-surface": GaSurface;
  }
}
