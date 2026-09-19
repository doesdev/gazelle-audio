// <ga-strip device-id="…" mixer="0" strip="3|master" [label="Vox"] [color="#rrggbb"] [inactive] [meter="off"] [input-group input-channel] [compact]>: one
// mixer channel strip, in the dense style of DAW mixers. Top to bottom: send (Studio+ Mix 1), pan,
// mute/solo/link, a fader with its dB scale beside a meter with a clip light, level and peak
// readouts, and a coloured name bar. Values and scales come from the store's MixerModel (the vendor
// panels' own scales). `label` names the strip; `inactive` disables its controls (a channel with no
// input or main mix); `meter="off"` blanks its meter (a channel not in the selected mix). The meter
// shows the channel's input, named by `input-group` and `input-channel`: the signal arriving, before
// the fader, since the Quadro's mixer meters cannot be moved off Mix 1 (hardware, 2026-09-16).
// `compact` is the mixer dock's slim strip: fader, meter with its clip light, mute and solo, level,
// name and the doubled badge, without pan, send, link or the peak readout.
// Attributes are read when the strip renders: change them by replacing the strip.

import { h } from "../core/dom.ts";
import { animateMeter, METER_FLOOR } from "./meter-motion.ts";
import { faderPosition, formatLevel, formatPan, panAtPosition, formatSend, LEVEL_MAX, levelAtFaderPosition, meterDeflection, METER_MARKS, PAN_CENTRE, PAN_MAX, PAN_MIN, SEND_MAX, type StripId } from "../store/mixer.ts";
import { bindControl, levelReset } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { linkButton } from "./link-bar.ts";

/** Where a double-click puts a fader or a send: -20 dB, a safe level (the user, 2026-09-18). Ctrl+click is unity. */
const SAFE_LEVEL = 20;

/** The fader cap's height; its centre line marks the level. */
const FADER_CAP_PX = 24;

/** Scale marks on the fader, closer together towards the floor as its audio taper draws them. */
const FADER_MARKS = [0, 5, 10, 20, 30, 40, 60, 90];

/** A peak in dB below full scale, as the readouts show it. */
const peakText = (db: number) => (db >= METER_FLOOR ? "< -60" : db < 0.5 ? "0" : `-${Math.round(db)}`);

export class GaStrip extends GaElement {
  static override styles = [
    sheet(`
      /* The mixer page sets the host's width; the strip compacts itself when that is narrow. */
      :host { display: flex; container: strip / inline-size; }
      .strip {
        display: flex;
        flex: 1;
        flex-direction: column;
        gap: 4px;
        min-width: 0;
        padding: 4px 3px 0;
        border-radius: 3px;
        background: var(--ga-surface-raised);
      }
      .row { display: flex; gap: 2px; }
      .toggle { flex: 1; min-width: 0; min-height: 18px; padding: 0; font-size: 10px; font-weight: 700; }
      .mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .solo[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
      .link[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .link[data-drafting] { outline: 1px dashed var(--ga-accent); outline-offset: -2px; }
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
      /* Short ticks at the edges only, so the centre mark never strikes through the value; at centre the value says "C" and the ticks go. */
      .bar .centre { position: absolute; top: 0; bottom: 0; left: 50%; width: 1px; background: linear-gradient(var(--ga-border-strong) 0 2px, transparent 2px calc(100% - 2px), var(--ga-border-strong) calc(100% - 2px)); }
      .bar[data-centred] .centre { display: none; }
      .bar .value { position: absolute; inset: 0; font-size: 9px; line-height: 13px; text-align: center; pointer-events: none; font-variant-numeric: tabular-nums; }
      /* The fader takes the height left under the channel head; its floor keeps it usable in short windows. */
      .level-area { display: grid; grid-template-columns: 16px 22px 1fr; gap: 3px; flex: 1; min-height: 100px; }
      :host([strip="master"]) .level-area { grid-template-columns: 18px 1fr; }
      .scale { position: relative; font-size: 8px; color: var(--ga-text-muted); font-variant-numeric: tabular-nums; }
      .scale span { position: absolute; right: 0; transform: translateY(-50%); }
      .fader { position: relative; cursor: ns-resize; touch-action: none; outline: none; }
      .groove { position: absolute; top: 0; bottom: 0; left: 50%; width: 4px; margin-left: -2px; border-radius: 2px; background: var(--ga-fader-track); }
      .cap {
        position: absolute;
        left: 0;
        right: 0;
        height: ${FADER_CAP_PX}px;
        top: calc((100% - ${FADER_CAP_PX}px) * var(--position, 0));
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
      /* Plasma style: a soft glow along the bar's top edge, and a bright peak marker above it. */
      .meter .mask::after { content: ""; position: absolute; left: 0; right: 0; bottom: -2px; height: 2px; background: var(--ga-text-primary); opacity: 0.35; filter: blur(1.5px); pointer-events: none; }
      .meter .peak-mark { position: absolute; left: 0; right: 0; height: 2px; margin-bottom: -1px; background: var(--ga-text-primary); opacity: 0.85; box-shadow: 0 0 4px var(--ga-text-primary); pointer-events: none; }
      .meter .peak-mark[hidden] { display: none; }
      .meter .tick { position: absolute; left: 0; right: 0; height: 1px; background: rgb(0 0 0 / 0.4); }
      /* One signal reaching the mix twice: small, in the warning colour, the reason in its title. */
      .doubled {
        align-self: center;
        padding: 0 4px;
        border-radius: 3px;
        background: var(--ga-notice-warning);
        color: var(--ga-surface-inset);
        font-size: 10px;
        font-weight: 600;
        cursor: help;
      }
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
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      :host([inactive]) .name { background: var(--ga-control-disabled); color: var(--ga-control-disabled-text); }
      /* Narrow strips drop the fader scale (the readout still shows the level) and widen the meter's share. */
      @container strip (max-width: 60px) {
        :host(:not([strip="master"])) .scale { display: none; }
        :host(:not([strip="master"])) .level-area { grid-template-columns: 1fr 12px; }
        .readout { font-size: 9px; padding: 1px 0; }
      }
      @container strip (min-width: 90px) {
        .level-area { grid-template-columns: 20px 1fr 1fr; }
        :host([strip="master"]) .level-area { grid-template-columns: 20px 1fr; }
      }
      /* Compact (the mixer dock): no pan, send or link, one readout, and the scale as bare ticks
         beside the fader, drawn on the cap's travel like the numbered one. */
      :host([compact]) .strip { gap: 3px; padding: 3px 2px 0; }
      :host([compact]) .level-area { grid-template-columns: 4px 1fr 6px; gap: 2px; min-height: 60px; }
      :host([compact][strip="master"]) .level-area { grid-template-columns: 14px 1fr; }
      :host([compact]:not([strip="master"])) .scale { display: block; }
      :host([compact]:not([strip="master"])) .scale span { left: 0; width: 4px; height: 1px; font-size: 0; background: var(--ga-text-muted); }
      :host([compact][strip="master"]) .scale { font-size: 7px; }
      :host([compact]) .toggle { min-height: 16px; font-size: 9px; }
      :host([compact]) .readout { padding: 1px 0; font-size: 9px; }
      :host([compact]) .name { margin: 0 -2px; padding: 2px 1px; font-size: 10px; }
      /* The doubled badge stays on a dock strip: a mix summing one input twice matters wherever it is ridden. */
      :host([compact]) .doubled { padding: 0 3px; font-size: 9px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const mixer = store.mixer(deviceId, Number(this.getAttribute("mixer") ?? "0"));
    const stripAttribute = this.getAttribute("strip") ?? "0";
    const id: StripId = stripAttribute === "master" ? "master" : Number(stripAttribute);
    const name = this.getAttribute("label") ?? "";
    const label = name !== "" ? name : id === "master" ? "Master" : `Strip ${id + 1}`;
    const testId = id === "master" ? "master" : String(id);
    const inactive = this.hasAttribute("inactive");
    const compact = this.hasAttribute("compact");
    const metered = this.getAttribute("meter") !== "off";
    const inputGroup = this.getAttribute("input-group");
    const inputMeter =
      inputGroup === null ? undefined : store.inputMeter(deviceId, { group: Number(inputGroup), channel: Number(this.getAttribute("input-channel") ?? "0") });
    const enabled = () => store.connected.peek() && !inactive;
    const state = mixer.strip(id);

    const cap = h("div", { class: "cap" });
    const fader = h("div", { class: "fader", role: "slider", tabindex: 0, "aria-label": `${label} level`, "aria-valuemin": -LEVEL_MAX, "aria-valuemax": 0, "data-testid": `fader-${testId}`, "data-explain": id === "master" ? "strip.master-fader" : "strip.fader", "data-explain-name": label }, h("div", { class: "groove" }), cap);
    bindControl(fader, { axis: "y", min: 0, max: LEVEL_MAX, up: -1, page: 6, reset: SAFE_LEVEL, get: () => state.peek().level, set: (v) => mixer.setLevel(id, v), enabled, valueAt: levelAtFaderPosition, inset: FADER_CAP_PX / 2, level: levelReset(store.doubleClickUnity, (fn) => this.watch(fn)) });
    // Marks share the cap's travel, so the cap's centre line sits on the mark for its level.
    const scale = h("div", { class: "scale", "aria-hidden": "true" }, FADER_MARKS.map((mark) => h("span", { style: `top: calc(${FADER_CAP_PX / 2}px + (100% - ${FADER_CAP_PX}px) * ${faderPosition(mark)})` }, mark === 0 ? "0" : `-${mark}`)));
    const levelReadout = h("span", { class: "readout", "data-testid": `level-${testId}`, "data-explain": "strip.level" });
    const mute = h("button", { class: "toggle mute", type: "button", "aria-label": `${label} mute`, "data-explain": id === "master" ? "strip.master-mute" : "strip.mute", "data-explain-name": label, "on:click": () => mixer.toggleMute(id) }, "M");

    const buttons: HTMLElement[] = [mute];
    const top: HTMLElement[] = [];
    const levelArea = h("div", { class: "level-area" }, scale, fader);
    const readouts = h("div", { class: "readouts" }, levelReadout);

    if (id !== "master") {
      const solo = h("button", { class: "toggle solo", type: "button", "aria-label": `${label} solo`, "data-explain": "strip.solo", "data-explain-name": label, "on:click": () => mixer.toggleSolo(id) }, "S");
      buttons.push(solo);
      this.watch(() => {
        solo.setAttribute("aria-pressed", String(state.value.solo));
      });
      if (!compact) {
        // Channel links are the workspace's; the badge opens the link bar on the Mixer page.
        buttons.push(linkButton((fn) => this.watch(fn), "mixer", deviceId, id, `mixer-link-${id}`, "toggle link"));

        const panFill = h("div", { class: "fill" });
        const panValue = h("span", { class: "value" });
        const pan = h("div", { class: "bar pan", role: "slider", tabindex: 0, "aria-label": `${label} pan`, "aria-valuemin": PAN_MIN - PAN_CENTRE, "aria-valuemax": PAN_MAX - PAN_CENTRE, "data-testid": `pan-${testId}`, "data-explain": "strip.pan", "data-explain-name": label }, h("div", { class: "centre" }), panFill, panValue);
        // While the mix is mono the device is centred; the control shows and moves the pan it returns to.
        bindControl(pan, { axis: "x", min: PAN_MIN, max: PAN_MAX, up: 1, page: 5, reset: PAN_CENTRE, valueAt: panAtPosition, get: () => mixer.monoPan(id) ?? state.peek().pan, set: (v) => mixer.setPan(id, v), enabled });
        top.push(pan);
        this.watch(() => {
          const monoPan = mixer.monoPan(id);
          const shownPan = monoPan ?? state.value.pan;
          const position = ((shownPan - PAN_MIN) / (PAN_MAX - PAN_MIN)) * 100;
          panFill.style.cssText = position >= 50 ? `left: 50%; width: ${position - 50}%` : `left: ${position}%; width: ${50 - position}%`;
          panValue.textContent = formatPan(shownPan);
          pan.setAttribute("aria-valuenow", String(shownPan - PAN_CENTRE));
          pan.setAttribute("aria-valuetext", formatPan(shownPan));
          pan.toggleAttribute("data-mono", monoPan !== undefined);
          pan.toggleAttribute("data-centred", shownPan === PAN_CENTRE);
          pan.title = monoPan === undefined ? "" : "This mix is mono: the channel is centred, and returns to this pan when mono is turned off";
        });
      }

      if (mixer.hasReverbSend && !compact) {
        const sendFill = h("div", { class: "fill" });
        const sendValue = h("span", { class: "value" });
        // Send is attenuation like the fader: 0 dB at the right, off (−inf) at the left.
        const send = h("div", { class: "bar send", role: "slider", tabindex: 0, "aria-label": `${label} send`, "aria-valuemin": -SEND_MAX, "aria-valuemax": 0, "data-explain": "strip.send", "data-explain-name": label }, sendFill, sendValue);
        bindControl(send, { axis: "x", min: SEND_MAX, max: 0, up: -1, page: 6, reset: SEND_MAX, get: () => state.peek().send, set: (v) => mixer.setSend(id, v), enabled, level: levelReset(store.doubleClickUnity, (fn) => this.watch(fn), 0, "off") });
        top.unshift(h("span", { class: "caption" }, "Send"), send);
        this.watch(() => {
          const value = state.value.send;
          sendFill.style.cssText = `left: 0; width: ${((SEND_MAX - Math.min(SEND_MAX, value)) / SEND_MAX) * 100}%`;
          sendValue.textContent = formatSend(value);
          send.setAttribute("aria-valuenow", String(-value));
          send.setAttribute("aria-valuetext", formatSend(value));
        });
      }

      const clip = h("button", { class: "clip", type: "button", "aria-label": `${label} clip; select to clear`, "data-explain": "strip.clip", "data-explain-name": label, "on:click": () => inputMeter?.clearClip() });
      const mask = h("div", { class: "mask" });
      const peakMark = h("div", { class: "peak-mark", hidden: "" });
      const meter = h(
        "div",
        { class: "meter", "data-testid": `meter-${testId}`, "data-explain": "strip.meter", "data-explain-name": label },
        h("div", { class: "gradient" }),
        mask,
        peakMark,
        METER_MARKS.map((mark) => h("div", { class: "tick", style: `bottom: ${meterDeflection(mark)}%` })),
      );
      const peakReadout = h("span", { class: "readout muted", title: "Peak, dB below full scale", "data-explain": "strip.peak" });
      levelArea.append(h("div", { class: "meter-column" }, clip, meter));
      // Compact strips keep the level readout only; the meter's held peak marker still shows.
      if (!compact) readouts.append(peakReadout);

      if (metered && inputMeter !== undefined) {
        // Smoothed as a plasma meter: quick to rise, falling back steadily, with a held peak.
        this.onDisconnect(
          animateMeter(inputMeter.level, (motion) => {
            mask.style.height = `${100 - meterDeflection(motion.level)}%`;
            peakMark.hidden = motion.peak >= METER_FLOOR;
            peakMark.style.bottom = `${meterDeflection(motion.peak)}%`;
            peakReadout.textContent = peakText(motion.peak);
          }),
        );
      } else {
        mask.style.height = "100%";
        // Blank rather than a mark, holding the row's height; the meter's title says why it reads nothing.
        peakReadout.textContent = "\u00a0";
      }
      if (!metered) meter.title = "This channel is not in the selected mix";
      else if (inputMeter === undefined) meter.title = "This input reports no meter";
      // A meter that reads nothing says why: an effect chain may be empty or simply unread.
      else {
        this.watch(() => {
          meter.title = inputMeter.note.value;
        });
      }
      this.watch(() => {
        clip.toggleAttribute("data-on", inputMeter?.clipped.value === true);
      });
      // The same audio can reach one mix twice: two channels on one input, or a channel whose empty
      // effect chain passes the other's input through (the user, at the hardware, 2026-09-18).
      const doubled = store.doubledFeed(deviceId, Number(this.getAttribute("mixer") ?? "0"), id);
      const badge = h("div", { class: "doubled", "data-testid": `doubled-${testId}`, hidden: "", "data-explain": "strip.doubled" }, "\u00d72");
      top.unshift(badge);
      this.watch(() => {
        const message = doubled.value;
        badge.hidden = message === undefined;
        badge.title = message ?? "";
        badge.setAttribute("aria-label", message ?? "");
      });
      this.watch(() => {
        const colours = Math.max(1, store.theme.value.palette.length);
        // A channel's colour (the `color` attribute: its group's, its own or its input's) wins over the theme palette.
        const own = this.getAttribute("color");
        this.style.setProperty("--strip-colour", own !== null && own !== "" ? own : `var(--ga-channel-palette-${Math.floor(id / 2) % colours})`);
      });
    }

    this.root.replaceChildren(
      h("div", { class: "strip" }, top, h("div", { class: "row" }, buttons), levelArea, readouts, h("div", { class: "name", title: label, "data-explain": "strip.name", "data-explain-name": label }, name !== "" ? name : id === "master" ? "Master" : String(id + 1))),
    );

    this.watch(() => {
      const s = state.value;
      cap.style.setProperty("--position", String(faderPosition(s.level)));
      fader.setAttribute("aria-valuenow", String(-s.level));
      fader.setAttribute("aria-valuetext", formatLevel(s.level));
      levelReadout.textContent = formatLevel(s.level);
      mute.setAttribute("aria-pressed", String(s.mute));
    });
    this.watch(() => {
      const usable = store.connected.value && !inactive;
      for (const button of this.root.querySelectorAll("button")) button.disabled = !usable;
      for (const control of this.root.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!usable));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-strip": GaStrip;
  }
}
