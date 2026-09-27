// <ga-recording-widget>: the recording widget, the `#/widget` route. In the desktop app it is a small
// frameless window that stays on top of everything else (around 320 by 140); in a browser it is
// just the page, which is how it is tried and tested.
//
// Top to bottom:
// - the light, the state (Off, Armed, Recording, red while recording) and the time since Record;
// - the preset, the pre-roll held, and auto-arm when it is on (pressing it, twice, turns it off);
// - one big button for what can be done now (Arm, Record or Stop), and Disarm, small, beside it;
// - a warning line: what was lost, a disk getting full, a refusal, the connection gone.
//
// It is dragged by its body: a press anywhere that is not a button asks the window to move
// (`toWindow("drag")`), and Windows' own move loop takes it from there. It has no chrome of the app:
// no header, no sidebar, no dock. Everything it shows and presses is the one recording model
// (`store/recording.ts`) the Recording page and the phone's transport use.

import { h } from "../core/dom.ts";
import { signal } from "../core/signal.ts";
import { autoArmShort, autoArmText, clockText, recordingModel, secondsText, stateLabel, warnings } from "../store/recording.ts";
import { bindConfirm } from "./controls.ts";
import { applyTheme, GaElement, sheet, useStore } from "./element.ts";
import { bindArm, followWorkspace, inWindow, toWindow, type TransportHost } from "./recording-transport.ts";

/** What a press on these may not start: a drag. */
const PRESSABLE = "button, select, input, a[href], [role='button']";

export class GaRecordingWidget extends GaElement {
  static override styles = [
    sheet(`
      :host {
        display: grid;
        grid-template-rows: auto auto minmax(0, 1fr) auto;
        gap: 4px;
        box-sizing: border-box;
        height: 100vh;
        height: 100dvh;
        padding: 6px 8px 6px 10px;
        overflow: hidden;
        background: var(--ga-surface-panel);
        border: 1px solid var(--ga-border-subtle);
        color: var(--ga-text-primary);
        user-select: none;
        -webkit-user-select: none;
        cursor: default;
        font-size: 12px;
      }
      :host([data-state="recording"]) { border: 2px solid var(--ga-notice-error); padding: 5px 7px 5px 9px; }
      .top { display: flex; align-items: center; gap: 8px; min-width: 0; }
      .lamp { flex: none; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--ga-text-muted); box-sizing: border-box; }
      :host([data-state="armed"]) .lamp, :host([data-state="arming"]) .lamp { border-color: var(--ga-notice-error); animation: ga-armed 1.2s ease-in-out infinite; }
      :host([data-state="recording"]) .lamp { border-color: var(--ga-notice-error); background: var(--ga-notice-error); box-shadow: 0 0 8px var(--ga-notice-error); }
      @keyframes ga-armed { 50% { opacity: 0.35; } }
      @media (prefers-reduced-motion: reduce) { .lamp { animation: none !important; } }
      .label { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: clamp(16px, 6.5vw, 30px); font-weight: 700; line-height: 1; white-space: nowrap; }
      :host([data-state="recording"]) .label { color: var(--ga-notice-error); }
      :host([data-connected="false"]) .label { color: var(--ga-text-muted); }
      .top .spacer { flex: 1; }
      .clock { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: clamp(16px, 6.5vw, 30px); line-height: 1; font-variant-numeric: tabular-nums; color: var(--ga-text-secondary); white-space: nowrap; }
      :host([data-state="recording"]) .clock { color: var(--ga-text-primary); }
      .clock[hidden], .close[hidden] { display: none; }
      .close { flex: none; min-width: 0; min-height: 0; width: 20px; height: 20px; padding: 0; font-size: 14px; line-height: 1; color: var(--ga-text-muted); background: transparent; border-color: transparent; }
      .close:hover { color: var(--ga-text-primary); background: var(--ga-control-hover); }
      .info { display: flex; align-items: center; gap: 6px; min-width: 0; color: var(--ga-text-secondary); white-space: nowrap; }
      .info .text { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
      .auto { flex: none; min-height: 0; height: 18px; padding: 0 6px; font-size: 11px; border-radius: 9px; color: var(--ga-accent); border-color: var(--ga-accent); background: transparent; }
      .auto[hidden] { display: none; }
      .auto[data-armed] { color: var(--ga-notice-error); border-color: var(--ga-notice-error); }
      .buttons { display: flex; align-items: stretch; gap: 6px; min-height: 0; }
      .buttons button { min-height: 0; }
      .main { flex: 1; font-size: clamp(13px, 5vw, 22px); font-weight: 700; }
      .main[hidden], .disarm[hidden] { display: none; }
      .record { border-color: var(--ga-notice-error); color: var(--ga-notice-error); }
      .record:not(:disabled):hover { background: var(--ga-notice-error); color: var(--ga-text-inverse); }
      .disarm { flex: none; padding: 0 8px; font-size: 11px; color: var(--ga-text-secondary); background: transparent; }
      .disarm[data-armed] { outline: 2px dashed var(--ga-notice-error); outline-offset: 1px; color: var(--ga-notice-error); }
      .warn { min-height: 1.3em; margin: 0; color: var(--ga-notice-warning); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-size: 11px; line-height: 1.3; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const model = recordingModel(store);
    const host: TransportHost = { watch: (fn) => this.watch(fn), onDisconnect: (fn) => this.onDisconnect(fn) };
    this.onDisconnect(model.activate());
    this.watch(() => applyTheme(store));
    document.title = "Gazelle recording";

    // A clock for what counts down (auto-arm's next try), once a second.
    const now = signal(Date.now());
    const tick = setInterval(() => (now.value = Date.now()), 1000);
    this.onDisconnect(() => clearInterval(tick));
    this.onDisconnect(followWorkspace(store));

    const lamp = h("span", { class: "lamp", role: "img", "aria-label": "Off", "data-explain": "recording.lamp" });
    const label = h("span", { class: "label", "data-testid": "widget-state", "data-explain": "recording.state" }, "Off");
    const clock = h("span", { class: "clock", "data-testid": "widget-elapsed", "data-explain": "recording.elapsed", hidden: true }, "0:00.0");
    const close = h("button", { type: "button", class: "close", "aria-label": "Close the recording widget", title: "Close the recording widget", "data-testid": "widget-close", "data-explain": "recording.widget-close", hidden: !inWindow() }, "×");
    close.addEventListener("click", () => toWindow("close"));

    const info = h("span", { class: "text", "data-testid": "widget-info", "data-explain": "recording.widget-info" });
    const auto = h("button", { type: "button", class: "auto", "data-testid": "widget-auto-arm", "data-explain": "recording.auto-arm-off", hidden: true }, "Auto");
    let autoLabel = "Auto";
    this.onDisconnect(bindConfirm(auto, () => autoLabel, () => void model.setSettings({ auto_arm: false }), () => !store.phone));

    const arm = h("button", { type: "button", class: "main arm", "data-testid": "widget-arm", "data-explain": "recording.arm" }, "Arm");
    const offered = bindArm(host, arm);
    const record = h("button", { type: "button", class: "main record", "data-testid": "widget-record", "data-explain": "recording.record" }, "● Record");
    record.addEventListener("click", () => void model.record());
    const stop = h("button", { type: "button", class: "main stop", "data-testid": "widget-stop", "data-explain": "recording.stop" }, "■ Stop");
    stop.addEventListener("click", () => void model.stop());
    const disarm = h("button", { type: "button", class: "disarm", "data-testid": "widget-disarm", "data-explain": "recording.disarm" }, "Disarm");
    this.onDisconnect(bindConfirm(disarm, "Disarm", () => void model.disarm(true), () => model.status.peek()?.state === "recording"));
    const warn = h("p", { class: "warn", role: "status", "data-testid": "widget-warning" });

    this.root.replaceChildren(
      h("div", { class: "top" }, lamp, label, h("span", { class: "spacer" }), clock, close),
      h("div", { class: "info" }, info, auto),
      h("div", { class: "buttons" }, arm, record, stop, disarm),
      warn,
    );

    // The body moves the window; the buttons are pressed.
    this.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      const pressed = event.composedPath().some((node) => node instanceof Element && node.matches(PRESSABLE));
      if (!pressed) toWindow("drag");
    });

    this.watch(() => {
      const status = model.status.value;
      const busy = model.busy.value;
      const connected = store.connected.value;
      void store.workspace.value;
      const state = status?.state ?? "off";
      this.dataset["state"] = state;
      this.dataset["connected"] = String(connected);
      label.textContent = connected ? stateLabel(status) : "Not connected";
      lamp.setAttribute("aria-label", stateLabel(status));
      clock.hidden = state !== "recording";
      clock.textContent = clockText(status?.take?.elapsed_seconds);

      const { name, problem } = offered();
      const preroll = status?.preroll === undefined ? undefined : state === "recording" ? `${secondsText(status.preroll.held_seconds)} pre-roll` : `${status.preroll.held_seconds.toFixed(1)} of ${status.preroll.preroll_seconds.toFixed(0)} s held`;
      const preset = status?.preset?.name ?? name ?? status?.auto_arm?.preset_name ?? "No preset yet";
      info.textContent = preroll === undefined ? preset : `${preset} · ${preroll}`;
      info.title = info.textContent;

      const off = state === "off";
      arm.hidden = !off && state !== "arming";
      arm.disabled = !connected || busy !== undefined || state === "arming" || problem !== undefined;
      record.hidden = state !== "armed";
      record.disabled = !connected || busy !== undefined;
      stop.hidden = state !== "recording";
      stop.disabled = !connected || busy !== undefined;
      disarm.hidden = off || state === "arming";
      disarm.disabled = !connected || busy === "disarm" || state === "disarming";

      const short = autoArmShort(status?.auto_arm, now.value);
      auto.hidden = short === undefined;
      autoLabel = short ?? "Auto";
      if (!auto.hasAttribute("data-armed")) auto.textContent = autoLabel;
      auto.title = `${autoArmText(status?.auto_arm, now.value) ?? ""}${store.phone ? "" : " Press twice to turn auto-arm off."}`;
      auto.disabled = store.phone || !connected;

      const said = connected ? warnings(model.problem.value, status) : ["The server is not connected. The buttons wait for it to come back."];
      if (status?.auto_arm?.phase === "backing_off") said.push(autoArmText(status.auto_arm, now.value) ?? "");
      if (said.length === 0 && off && problem !== undefined) said.push(problem);
      warn.textContent = said.join(" ");
      warn.title = warn.textContent;
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-recording-widget": GaRecordingWidget;
  }
}
