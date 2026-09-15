// <ga-control-room>: the right zone's monitor panel (decision P56). It shows the device on the
// current page, or the first of known model, as a <ga-monitor device-id="…">: the monitor output's
// volume, mute and (Quadro) dim, a mono badge where the device reports one, and the Studio+ talk
// button. The panel uses the same OutputsModel as the Outputs page, so the two stay in step.

import { h } from "../core/dom.ts";
import { formatVolume, VOLUME_MAX } from "../store/outputs.ts";
import { bindControl } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { route } from "./router.ts";

/** The monitor output's id on both models. */
const MONITOR = 0;

export class GaControlRoom extends GaElement {
  static override styles = [sheet(`:host { display: block; }`)];

  protected override render(): void {
    const store = useStore();
    let shown: string | undefined;
    this.watch(() => {
      const current = route.value;
      const known = store.devices.value.filter((d) => d.family !== null);
      const device = known.find((d) => d.id === current.id) ?? known[0];
      if (device?.id === shown && this.root.childElementCount > 0) return;
      shown = device?.id;
      this.root.replaceChildren(device === undefined ? h("p", { class: "placeholder" }, "No device of known model is connected.") : h("ga-monitor", { "device-id": device.id }));
    });
  }
}

export class GaMonitor extends GaElement {
  static override styles = [
    sheet(`
      :host { display: grid; gap: 6px; }
      .device { font-size: 11px; color: var(--ga-text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .head { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
      .label { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; }
      .mono { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-inverse); background: var(--ga-accent); }
      .volume { position: relative; height: 26px; border: 1px solid var(--ga-border-subtle); border-radius: 3px; background: var(--ga-surface-inset); cursor: ew-resize; touch-action: none; outline: none; }
      .volume:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .volume .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
      .volume .value { position: absolute; inset: 0; font-size: 12px; line-height: 24px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
      .volume[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .buttons { display: flex; gap: 4px; }
      .buttons button { flex: 1; min-width: 0; font-size: 11px; font-weight: 700; }
      .mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .dim[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .talk[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const outputs = store.outputs(deviceId);
    this.onDisconnect(outputs.activate());
    const state = outputs.state(MONITOR);
    const enabled = () => store.connected.peek();
    const model = store.devices.peek().find((d) => d.id === deviceId)?.model ?? deviceId;

    const fill = h("div", { class: "fill" });
    const value = h("span", { class: "value" });
    const volume = h("div", { class: "volume", role: "slider", tabindex: 0, "aria-label": "Monitor volume", "aria-valuemin": -VOLUME_MAX, "aria-valuemax": 0, "data-testid": "cr-volume" }, fill, value);
    bindControl(volume, { axis: "x", min: VOLUME_MAX, max: 0, up: -1, page: 6, reset: 30, get: () => state.peek().volume, set: (v) => outputs.setVolume(MONITOR, v), enabled });
    const mute = h("button", { type: "button", class: "mute", "data-testid": "cr-mute", "aria-label": "Monitor mute", "on:click": () => outputs.setMute(MONITOR, !state.peek().mute) }, "Mute");
    const dim = outputs.outputs[MONITOR]?.dim ? h("button", { type: "button", class: "dim", "data-testid": "cr-dim", "aria-label": "Monitor dim", "on:click": () => outputs.setDim(MONITOR, !state.peek().dim) }, "Dim") : undefined;
    const talk = outputs.talkback === undefined ? undefined : h("button", { type: "button", class: "talk", "data-testid": "cr-talk", "aria-label": "Talkback", "on:click": () => outputs.setTalk(!outputs.talk.peek().on) }, "Talk");
    const mono = h("span", { class: "mono", title: "The device reports the monitor in mono", hidden: true }, "MONO");

    this.root.replaceChildren(
      h("div", { class: "device", title: model }, model),
      h("div", { class: "head" }, h("span", { class: "label" }, "Monitor"), mono),
      volume,
      h("div", { class: "buttons" }, mute, dim, talk),
    );

    this.watch(() => {
      const s = state.value;
      fill.style.width = `${((VOLUME_MAX - Math.min(VOLUME_MAX, Math.max(0, s.volume))) / VOLUME_MAX) * 100}%`;
      value.textContent = formatVolume(s.volume);
      volume.setAttribute("aria-valuenow", String(-s.volume));
      volume.setAttribute("aria-valuetext", formatVolume(s.volume));
      mute.setAttribute("aria-pressed", String(s.mute));
      dim?.setAttribute("aria-pressed", String(s.dim));
      mono.hidden = !s.mono;
    });
    if (talk !== undefined) this.watch(() => talk.setAttribute("aria-pressed", String(outputs.talk.value.on)));
    this.watch(() => {
      const connected = store.connected.value;
      for (const button of [mute, dim, talk]) if (button !== undefined) button.disabled = !connected;
      volume.setAttribute("aria-disabled", String(!connected));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-control-room": GaControlRoom;
    "ga-monitor": GaMonitor;
  }
}
