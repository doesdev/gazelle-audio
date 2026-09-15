// <ga-outputs device-id="…">: a device's hardware outputs, each with the discrete controls its vendor
// panel binds: volume (dB of attenuation, 0 dB to -inf), mute and, on the Quadro, dim. Values come
// from the device's reports; changes send set_volume, set_mute and set_dim (OutputsModel).

import { h } from "../core/dom.ts";
import { formatVolume, VOLUME_MAX, type OutputInfo, type OutputsModel } from "../store/outputs.ts";
import { bindControl } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href } from "./router.ts";

/** The vendor panels' starting volume, which a reset returns to. */
const VOLUME_RESET = 30;

export class GaOutputs extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 12px; }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .spacer { flex: 1; }
      .note { margin: 0; font-size: 11px; color: var(--ga-text-muted); }
      .last-sent { display: flex; min-width: 0; max-width: 100%; font-size: 11px; white-space: nowrap; }
      .last-sent code { min-width: 0; overflow: hidden; text-overflow: ellipsis; font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      .rows { display: grid; gap: 6px; max-width: 640px; }
      .output {
        display: grid;
        grid-template-columns: minmax(72px, 110px) minmax(0, 1fr) auto;
        align-items: center;
        gap: 10px;
        padding: 6px 8px;
        border-radius: 3px;
        background: var(--ga-surface-raised);
      }
      .name { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; }
      .volume {
        position: relative;
        height: 22px;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 3px;
        background: var(--ga-surface-inset);
        cursor: ew-resize;
        touch-action: none;
        outline: none;
      }
      .volume:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .volume .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
      .volume .value { position: absolute; inset: 0; font-size: 11px; line-height: 20px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
      .volume[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .toggles { display: flex; gap: 4px; }
      .toggles button { min-width: 44px; font-size: 11px; font-weight: 700; }
      .mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .dim[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      @media (max-width: 480px) {
        .output { grid-template-columns: 1fr auto; }
        .volume { grid-column: 1 / -1; grid-row: 2; }
      }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    if (store.topology(deviceId) === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so its outputs are not known.`));
      return;
    }
    const outputs = store.outputs(deviceId);
    this.onDisconnect(outputs.activate());
    const enabled = () => store.connected.peek();

    const devices = h("select", {
      "aria-label": "Device",
      "on:change": (event) => {
        location.hash = href({ page: "outputs", id: (event.target as HTMLSelectElement).value });
      },
    });
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent" });
    const note = h("p", { class: "note" });
    const rows = h("div", { class: "rows" }, outputs.outputs.map((output) => this.#row(outputs, output, enabled)));

    this.root.replaceChildren(h("div", { class: "bar" }, devices, h("span", { class: "spacer" }), lastSent), note, rows);

    this.watch(() => {
      const known = store.devices.value.filter((d) => d.family !== null);
      devices.replaceChildren(...known.map((d) => h("option", { value: d.id, selected: d.id === deviceId }, d.model ?? d.id)));
      devices.value = deviceId;
      devices.disabled = !store.connected.value;
    });
    this.watch(() => {
      note.textContent = outputs.outputs.length > 0 && !outputs.state(0).value.known ? "The device has not reported its output levels yet, so controls start at defaults and send when changed." : "";
    });
    this.watch(() => {
      const sent = store.lastSent.value;
      if (sent === undefined || sent.deviceId !== deviceId) {
        lastSent.textContent = store.server.value.dry_run ? "Dry run: nothing is written to the device" : "";
        return;
      }
      lastSent.replaceChildren(`${sent.dryRun ? "Dry run, would send" : "Sent"} ${sent.command}: `, h("code", { title: sent.hex }, sent.hex));
    });
    this.watch(() => {
      const connected = store.connected.value;
      for (const button of this.root.querySelectorAll<HTMLButtonElement>("button[data-control]")) button.disabled = !connected;
      for (const control of this.root.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!connected));
    });
  }

  #row(outputs: OutputsModel, output: OutputInfo, enabled: () => boolean): HTMLElement {
    const state = outputs.state(output.id);
    const fill = h("div", { class: "fill" });
    const value = h("span", { class: "value" });
    const volume = h(
      "div",
      { class: "volume", role: "slider", tabindex: 0, "aria-label": `${output.name} volume`, "aria-valuemin": -VOLUME_MAX, "aria-valuemax": 0, "data-testid": `out-volume-${output.id}` },
      fill,
      value,
    );
    bindControl(volume, { axis: "x", min: VOLUME_MAX, max: 0, up: -1, page: 6, reset: VOLUME_RESET, get: () => state.peek().volume, set: (v) => outputs.setVolume(output.id, v), enabled });
    const mute = h("button", { type: "button", class: "mute", "data-control": "", "data-testid": `out-mute-${output.id}`, "aria-label": `${output.name} mute`, "on:click": () => outputs.setMute(output.id, !state.peek().mute) }, "Mute");
    const dim = output.dim ? h("button", { type: "button", class: "dim", "data-control": "", "data-testid": `out-dim-${output.id}`, "aria-label": `${output.name} dim`, "on:click": () => outputs.setDim(output.id, !state.peek().dim) }, "Dim") : undefined;

    this.watch(() => {
      const s = state.value;
      fill.style.width = `${((VOLUME_MAX - Math.min(VOLUME_MAX, Math.max(0, s.volume))) / VOLUME_MAX) * 100}%`;
      value.textContent = formatVolume(s.volume);
      volume.setAttribute("aria-valuenow", String(-s.volume));
      volume.setAttribute("aria-valuetext", formatVolume(s.volume));
      mute.setAttribute("aria-pressed", String(s.mute));
      dim?.setAttribute("aria-pressed", String(s.dim));
    });

    return h("div", { class: "output", "data-testid": `output-${output.id}` }, h("span", { class: "name" }, output.name), volume, h("div", { class: "toggles" }, mute, dim));
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-outputs": GaOutputs;
  }
}
