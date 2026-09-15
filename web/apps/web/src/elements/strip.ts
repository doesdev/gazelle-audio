// <ga-strip device-id="…" mixer="0" strip="3|master">: one mixer channel strip, in the dense
// style of DAW mixers. Top to bottom: send (Studio+), pan, mute/solo/link, a fader with its dB
// scale beside a meter with a clip light, level and peak readouts, and a coloured name bar.
// Values and scales come from the store's MixerModel (the vendor panels' own scales).

import { h } from "../core/dom.ts";
import { formatLevel, formatPan, LEVEL_MAX, meterDeflection, METER_MARKS, PAN_CENTRE, PAN_MAX, PAN_MIN, SEND_MAX, type StripId } from "../store/mixer.ts";
import { GaElement, sheet, useStore } from "./element.ts";

interface ControlOptions {
  axis: "x" | "y";
  min: number;
  max: number;
  /** +1 when a larger value is "more" (pan right, send up); −1 for attenuation, where up means a smaller value. */
  up: 1 | -1;
  page: number;
  reset: number;
  get(): number;
  set(value: number): void;
  enabled(): boolean;
}

/** Pointer drag, wheel, double-click reset and keyboard control of a value along one axis. */
function bindControl(element: HTMLElement, options: ControlOptions): void {
  const valueAt = (event: PointerEvent) => {
    const rect = element.getBoundingClientRect();
    const fraction = options.axis === "y" ? (event.clientY - rect.top) / rect.height : (event.clientX - rect.left) / rect.width;
    return options.min + Math.min(1, Math.max(0, fraction)) * (options.max - options.min);
  };
  element.addEventListener("pointerdown", (event) => {
    if (!options.enabled() || event.button !== 0) return;
    element.setPointerCapture(event.pointerId);
    element.focus();
    options.set(valueAt(event));
    event.preventDefault();
  });
  element.addEventListener("pointermove", (event) => {
    if (element.hasPointerCapture(event.pointerId)) options.set(valueAt(event));
  });
  element.addEventListener("dblclick", () => {
    if (options.enabled()) options.set(options.reset);
  });
  element.addEventListener(
    "wheel",
    (event) => {
      if (!options.enabled()) return;
      event.preventDefault();
      options.set(options.get() + (event.deltaY < 0 ? options.up : -options.up));
    },
    { passive: false },
  );
  element.addEventListener("keydown", (event) => {
    if (!options.enabled()) return;
    const steps: Record<string, number> = { ArrowUp: options.up, ArrowRight: options.up, ArrowDown: -options.up, ArrowLeft: -options.up, PageUp: options.page * options.up, PageDown: -options.page * options.up };
    if (event.key in steps) options.set(options.get() + (steps[event.key] ?? 0));
    else if (event.key === "Home") options.set(options.min);
    else if (event.key === "End") options.set(options.max);
    else return;
    event.preventDefault();
  });
}

const FADER_MARKS = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90];

export class GaStrip extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; }
      .strip {
        display: flex;
        flex: 1;
        flex-direction: column;
        gap: 4px;
        width: 64px;
        padding: 4px 3px 0;
        border-radius: 3px;
        background: var(--ga-surface-raised);
      }
      :host([strip="master"]) .strip { width: 78px; }
      .row { display: flex; gap: 2px; }
      .toggle { flex: 1; min-width: 0; min-height: 18px; padding: 0; font-size: 10px; font-weight: 700; }
      .mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .solo[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
      .link[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .caption { font-size: 9px; color: var(--ga-text-muted); text-transform: uppercase; letter-spacing: 0.06em; text-align: center; }
      .bar {
        position: relative;
        height: 15px;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 2px;
        background: var(--ga-surface-inset);
        cursor: ew-resize;
        touch-action: none;
      }
      .bar .fill { position: absolute; top: 0; bottom: 0; background: var(--ga-accent); opacity: 0.75; }
      .bar .centre { position: absolute; top: 0; bottom: 0; left: 50%; width: 1px; background: var(--ga-border-strong); }
      .bar .value { position: absolute; inset: 0; font-size: 9px; line-height: 13px; text-align: center; pointer-events: none; font-variant-numeric: tabular-nums; }
      .level-area { display: grid; grid-template-columns: 16px 22px 1fr; gap: 3px; flex: 1; min-height: 180px; }
      :host([strip="master"]) .level-area { grid-template-columns: 18px 1fr; }
      .scale { position: relative; font-size: 8px; color: var(--ga-text-muted); font-variant-numeric: tabular-nums; }
      .scale span { position: absolute; right: 0; transform: translateY(-50%); }
      .fader { position: relative; cursor: ns-resize; touch-action: none; outline: none; }
      .groove { position: absolute; top: 0; bottom: 0; left: 50%; width: 4px; margin-left: -2px; border-radius: 2px; background: var(--ga-fader-track); }
      .cap {
        position: absolute;
        left: 0;
        right: 0;
        height: 24px;
        top: calc((100% - 24px) * var(--position, 0));
        border: 1px solid rgb(0 0 0 / 0.35);
        border-radius: 3px;
        background: linear-gradient(var(--ga-fader-cap-active), var(--ga-fader-cap));
        box-shadow: 0 1px 3px rgb(0 0 0 / 0.45);
      }
      .cap::after { content: ""; position: absolute; left: 3px; right: 3px; top: 50%; height: 2px; margin-top: -1px; background: rgb(0 0 0 / 0.45); }
      .fader:focus-visible .cap { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .fader[aria-disabled="true"], .bar[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .meter-column { display: flex; flex-direction: column; gap: 2px; }
      .clip { height: 5px; padding: 0; min-height: 5px; border: 0; border-radius: 1px; background: var(--ga-meter-background); }
      .clip[data-on] { background: var(--ga-meter-clip); cursor: pointer; }
      .meter { position: relative; flex: 1; overflow: hidden; border-radius: 2px; background: var(--ga-meter-background); }
      .meter .gradient { position: absolute; inset: 0; background: var(--mixer-meter-gradient, var(--ga-meter-gradient)); }
      .meter .mask { position: absolute; left: 0; right: 0; top: 0; height: 100%; background: var(--ga-meter-background); }
      .meter .tick { position: absolute; left: 0; right: 0; height: 1px; background: rgb(0 0 0 / 0.4); }
      .readouts { display: grid; gap: 2px; }
      .readout { min-width: 0; width: 100%; padding: 1px 2px; font-size: 10px; text-align: center; }
      .name {
        margin: 0 -3px;
        padding: 3px 2px;
        border-radius: 0 0 3px 3px;
        background: var(--strip-colour, var(--ga-section-header));
        color: var(--ga-text-inverse);
        font-family: "Josefin Sans Variable", system-ui, sans-serif;
        font-size: 12px;
        font-weight: 600;
        text-align: center;
      }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const mixer = store.mixer(deviceId, Number(this.getAttribute("mixer") ?? "0"));
    const stripAttribute = this.getAttribute("strip") ?? "0";
    const id: StripId = stripAttribute === "master" ? "master" : Number(stripAttribute);
    const label = id === "master" ? "Master" : `Strip ${id + 1}`;
    const testId = id === "master" ? "master" : String(id);
    const enabled = () => store.connected.peek();
    const state = mixer.strip(id);

    const cap = h("div", { class: "cap" });
    const fader = h("div", { class: "fader", role: "slider", tabindex: 0, "aria-label": `${label} level`, "aria-valuemin": -LEVEL_MAX, "aria-valuemax": 0, "data-testid": `fader-${testId}` }, h("div", { class: "groove" }), cap);
    bindControl(fader, { axis: "y", min: 0, max: LEVEL_MAX, up: -1, page: 6, reset: 0, get: () => state.peek().level, set: (v) => mixer.setLevel(id, v), enabled });
    const scale = h("div", { class: "scale", "aria-hidden": "true" }, FADER_MARKS.map((mark) => h("span", { style: `top: ${(mark / LEVEL_MAX) * 100}%` }, mark === 0 ? "0" : `-${mark}`)));
    const levelReadout = h("span", { class: "readout", "data-testid": `level-${testId}` });
    const mute = h("button", { class: "toggle mute", type: "button", "aria-label": `${label} mute`, "on:click": () => mixer.toggleMute(id) }, "M");

    const buttons: HTMLElement[] = [mute];
    const top: HTMLElement[] = [];
    const levelArea = h("div", { class: "level-area" }, scale, fader);
    const readouts = h("div", { class: "readouts" }, levelReadout);

    if (id !== "master") {
      const solo = h("button", { class: "toggle solo", type: "button", "aria-label": `${label} solo`, "on:click": () => mixer.toggleSolo(id) }, "S");
      const link = h("button", { class: "toggle link", type: "button", "aria-label": `Link strips ${id - (id % 2) + 1} and ${id - (id % 2) + 2}`, "on:click": () => mixer.toggleLink(id) }, "⇆");
      buttons.push(solo, link);

      const panFill = h("div", { class: "fill" });
      const panValue = h("span", { class: "value" });
      const pan = h("div", { class: "bar pan", role: "slider", tabindex: 0, "aria-label": `${label} pan`, "aria-valuemin": PAN_MIN - PAN_CENTRE, "aria-valuemax": PAN_MAX - PAN_CENTRE, "data-testid": `pan-${testId}` }, h("div", { class: "centre" }), panFill, panValue);
      bindControl(pan, { axis: "x", min: PAN_MIN, max: PAN_MAX, up: 1, page: 5, reset: PAN_CENTRE, get: () => state.peek().pan, set: (v) => mixer.setPan(id, v), enabled });
      top.push(pan);

      if (mixer.hasSend) {
        const sendFill = h("div", { class: "fill" });
        const sendValue = h("span", { class: "value" });
        const send = h("div", { class: "bar send", role: "slider", tabindex: 0, "aria-label": `${label} send (raw value, scale unverified)`, title: "Send: raw value; its scale is not known yet", "aria-valuemin": 0, "aria-valuemax": SEND_MAX }, sendFill, sendValue);
        bindControl(send, { axis: "x", min: 0, max: SEND_MAX, up: 1, page: 16, reset: 0, get: () => state.peek().send, set: (v) => mixer.setSend(id, v), enabled });
        top.unshift(h("span", { class: "caption" }, "Send"), send);
        this.watch(() => {
          const value = state.value.send;
          sendFill.style.cssText = `left: 0; width: ${(value / SEND_MAX) * 100}%`;
          sendValue.textContent = String(value);
          send.setAttribute("aria-valuenow", String(value));
        });
      }

      const clip = h("button", { class: "clip", type: "button", "aria-label": `${label} clip; select to clear`, "on:click": () => mixer.clearClip(id) });
      const mask = h("div", { class: "mask" });
      const meter = h(
        "div",
        { class: "meter", "data-testid": `meter-${testId}` },
        h("div", { class: "gradient" }),
        mask,
        METER_MARKS.map((mark) => h("div", { class: "tick", style: `bottom: ${meterDeflection(mark)}%` })),
      );
      const peakReadout = h("span", { class: "readout muted", title: "Peak, dB below full scale" });
      levelArea.append(h("div", { class: "meter-column" }, clip, meter));
      readouts.append(peakReadout);

      this.watch(() => {
        const s = state.value;
        solo.setAttribute("aria-pressed", String(s.solo));
        link.setAttribute("aria-pressed", String(s.linked));
        const position = ((s.pan - PAN_MIN) / (PAN_MAX - PAN_MIN)) * 100;
        panFill.style.cssText = position >= 50 ? `left: 50%; width: ${position - 50}%` : `left: ${position}%; width: ${50 - position}%`;
        panValue.textContent = formatPan(s.pan);
        pan.setAttribute("aria-valuenow", String(s.pan - PAN_CENTRE));
        pan.setAttribute("aria-valuetext", formatPan(s.pan));
      });
      this.watch(() => {
        const byte = mixer.meter(id).value;
        const deflection = byte === undefined ? 0 : meterDeflection(byte);
        mask.style.height = `${100 - deflection}%`;
        peakReadout.textContent = byte === undefined ? "—" : byte > 60 ? "< -60" : byte === 0 ? "0" : `-${byte}`;
      });
      this.watch(() => {
        clip.toggleAttribute("data-on", mixer.clipped(id).value);
      });
      this.watch(() => {
        const colours = Math.max(1, store.theme.value.palette.length);
        this.style.setProperty("--strip-colour", `var(--ga-channel-palette-${Math.floor(id / 2) % colours})`);
      });
    }

    this.root.replaceChildren(
      h("div", { class: "strip" }, top, h("div", { class: "row" }, buttons), levelArea, readouts, h("div", { class: "name" }, id === "master" ? "Master" : String(id + 1))),
    );

    this.watch(() => {
      const s = state.value;
      cap.style.setProperty("--position", String(s.level / LEVEL_MAX));
      fader.setAttribute("aria-valuenow", String(-s.level));
      fader.setAttribute("aria-valuetext", formatLevel(s.level));
      levelReadout.textContent = formatLevel(s.level);
      mute.setAttribute("aria-pressed", String(s.mute));
    });
    this.watch(() => {
      const connected = store.connected.value;
      for (const button of this.root.querySelectorAll("button")) button.disabled = !connected;
      for (const control of this.root.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!connected));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-strip": GaStrip;
  }
}
