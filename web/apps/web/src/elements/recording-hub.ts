// <ga-recording-hub>: the recording hub, the `#/hub` route. In the desktop app it is a window that
// fills a screen, to be read from across the room; in a browser it is just the page.
//
// - Across the top: the preset and the pre-roll, auto-arm, and the time of day.
// - The middle: the state, very large, and the time since Record, larger still; beside it the disk
//   time left at this rate and channel count, and the last few takes.
// - Below: every channel's meter, by the name Gazelle gives it.
// - At the bottom: Arm, Record and Stop as big targets. Disarm is small and off to the side, so it is
//   not hit by accident, and while a take is recording it asks for a second press, as everywhere.
//
// Space records and stops, as on the Recording page. Esc leaves full screen; in the desktop app the
// window then stays open as an ordinary one, and the Full screen button puts it back.

import { h } from "../core/dom.ts";
import { signal } from "../core/signal.ts";
import { autoArmText, clockText, diskLeftText, prerollFill, prerollText, recordingModel, stateLabel, takeLine, timeOfDay, warnings } from "../store/recording.ts";
import { bindConfirm } from "./controls.ts";
import { applyTheme, GaElement, sheet, useStore } from "./element.ts";
import { METRONOME_STYLES, metronomeCompact } from "./metronome-controls.ts";
import { autoArmOffButton, bindArm, CHANNEL_STYLES, channelMeters, followWorkspace, inWindow, spaceToggles, toWindow, type TransportHost } from "./recording-transport.ts";

/** How many takes the hub lists. */
export const HUB_TAKES = 4;

export class GaRecordingHub extends GaElement {
  static override styles = [
    sheet(`
      :host {
        display: grid;
        grid-template-rows: auto minmax(0, 1fr) auto auto auto;
        gap: 2vh;
        box-sizing: border-box;
        height: 100vh;
        height: 100dvh;
        padding: 3vh 3vw;
        overflow: hidden;
        background: var(--ga-surface-background);
        color: var(--ga-text-primary);
      }
      :host([data-state="recording"]) { box-shadow: inset 0 0 0 6px var(--ga-notice-error); }
      .bar { display: flex; align-items: center; gap: 2vw; min-width: 0; font-size: clamp(14px, 2.2vh, 28px); color: var(--ga-text-secondary); }
      .bar .preset { font-weight: 700; color: var(--ga-text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .bar .time { margin-left: auto; font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: clamp(20px, 5vh, 64px); font-variant-numeric: tabular-nums; color: var(--ga-text-primary); }
      .middle { display: grid; grid-template-columns: minmax(0, 1fr) minmax(260px, 30%); gap: 3vw; min-height: 0; }
      .big { display: grid; align-content: center; gap: 1vh; min-width: 0; }
      .state { display: flex; align-items: center; gap: 2.5vw; }
      .lamp { flex: none; width: 9vh; height: 9vh; border-radius: 50%; border: 1.4vh solid var(--ga-text-muted); box-sizing: border-box; }
      :host([data-state="armed"]) .lamp, :host([data-state="arming"]) .lamp { border-color: var(--ga-notice-error); animation: ga-armed 1.2s ease-in-out infinite; }
      :host([data-state="recording"]) .lamp { border-color: var(--ga-notice-error); background: var(--ga-notice-error); box-shadow: 0 0 4vh var(--ga-notice-error); }
      @keyframes ga-armed { 50% { opacity: 0.35; } }
      @media (prefers-reduced-motion: reduce) { .lamp { animation: none !important; } }
      .label { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: clamp(40px, 14vh, 190px); font-weight: 700; line-height: 1; letter-spacing: 0.01em; }
      :host([data-state="recording"]) .label { color: var(--ga-notice-error); }
      .elapsed { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: clamp(48px, 20vh, 260px); font-weight: 600; line-height: 1; font-variant-numeric: tabular-nums; color: var(--ga-text-muted); }
      :host([data-state="recording"]) .elapsed { color: var(--ga-text-primary); }
      .preroll { display: grid; gap: 0.8vh; max-width: 70%; font-size: clamp(13px, 2vh, 24px); color: var(--ga-text-secondary); }
      .preroll .track { height: 1.2vh; border-radius: 0.6vh; overflow: hidden; background: var(--ga-surface-inset); }
      .preroll .fill { height: 100%; width: calc(var(--fill, 0) * 100%); background: var(--ga-accent); transition: width 180ms linear; }
      :host([data-state="recording"]) .preroll .fill { background: var(--ga-notice-error); }
      .side { display: grid; align-content: start; gap: 3vh; min-width: 0; padding: 2vh 2vw; border-radius: 1vh; background: var(--ga-surface-panel); }
      .side h2 { margin: 0 0 0.6vh; font-size: clamp(12px, 1.8vh, 22px); font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; color: var(--ga-text-muted); }
      .disk { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: clamp(22px, 6vh, 80px); font-weight: 700; line-height: 1.05; }
      .disk-note { font-size: clamp(12px, 1.7vh, 20px); color: var(--ga-text-secondary); }
      .takes { display: grid; gap: 1vh; margin: 0; padding: 0; list-style: none; font-size: clamp(12px, 1.8vh, 22px); }
      .takes li { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--ga-text-secondary); }
      .takes li:first-child { color: var(--ga-text-primary); }
      ${CHANNEL_STYLES}
      ${METRONOME_STYLES}
      .side .metronome { gap: 1vh; font-size: clamp(12px, 1.8vh, 22px); }
      .side .metronome .said, .side .metronome .tempo .unit { font-size: inherit; }
      .side .metronome .start, .side .metronome .tempo button { min-height: clamp(32px, 5vh, 64px); min-width: clamp(40px, 5vh, 72px); font-size: clamp(14px, 2.4vh, 30px); }
      .side .metronome .tempo input { font-size: clamp(16px, 3vh, 40px); width: 4.2em; }
      .side .metronome .beats .dot { width: clamp(12px, 2.2vh, 30px); height: clamp(12px, 2.2vh, 30px); }
      .side .metronome .beats .dot.one { width: clamp(16px, 2.8vh, 38px); height: clamp(16px, 2.8vh, 38px); }
      :host([data-state="counting_in"]) .label { color: var(--ga-notice-warning); }
      :host([data-state="counting_in"]) .lamp { border-color: var(--ga-notice-warning); animation: ga-armed 0.5s ease-in-out infinite; }
      .meters .channels { grid-template-columns: repeat(auto-fill, minmax(min(560px, 100%), 1fr)); gap: 1vh 3vw; max-height: 26vh; overflow: auto; }
      .meters .channel { grid-template-columns: minmax(0, 48%) minmax(0, 1fr) 4.6em; font-size: clamp(13px, 2vh, 24px); }
      .meters .channel .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .meters .channel .level { height: 1.6vh; min-height: 10px; }
      .meters .channels .empty { font-size: clamp(13px, 2vh, 24px); }
      .controls { display: flex; align-items: stretch; gap: 1.5vw; }
      .controls .main { flex: 1 1 0; min-height: clamp(56px, 11vh, 150px); font-size: clamp(20px, 4.5vh, 60px); font-weight: 700; border-width: 2px; }
      .controls .main[hidden] { display: none; }
      .controls .main:disabled { opacity: 0.35; }
      .controls .record { border-color: var(--ga-notice-error); color: var(--ga-notice-error); }
      .controls .record:not(:disabled):hover { background: var(--ga-notice-error); color: var(--ga-text-inverse); }
      .controls .aside { display: grid; align-content: end; gap: 0.8vh; margin-left: 3vw; }
      .controls .aside button { min-height: clamp(28px, 4vh, 48px); font-size: clamp(12px, 1.8vh, 20px); }
      .controls .aside button[hidden] { display: none; }
      .disarm[data-armed] { outline: 3px dashed var(--ga-notice-error); outline-offset: 2px; color: var(--ga-notice-error); }
      .foot { display: flex; flex-wrap: wrap; align-items: center; gap: 0.6vh 2vw; min-height: 2.4vh; font-size: clamp(12px, 1.8vh, 22px); }
      .foot .warn { color: var(--ga-notice-warning); }
      .foot .auto { color: var(--ga-text-secondary); }
      .foot button { min-height: 0; padding: 2px 10px; font-size: clamp(12px, 1.6vh, 18px); }
      .foot [hidden] { display: none; }
      .keys { margin-left: auto; color: var(--ga-text-muted); }
      .disconnected { color: var(--ga-connection-reconnecting); font-weight: 700; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const model = recordingModel(store);
    const host: TransportHost = { watch: (fn) => this.watch(fn), onDisconnect: (fn) => this.onDisconnect(fn) };
    this.onDisconnect(model.activate());
    this.watch(() => applyTheme(store));
    document.title = "Gazelle recording hub";

    const now = signal(new Date());
    const tick = setInterval(() => (now.value = new Date()), 1000);
    this.onDisconnect(() => clearInterval(tick));
    this.onDisconnect(followWorkspace(store));

    const preset = h("span", { class: "preset", "data-testid": "hub-preset", "data-explain": "recording.preset-select" });
    const time = h("span", { class: "time", "data-testid": "hub-clock", "data-explain": "recording.hub-clock" });
    const lamp = h("span", { class: "lamp", role: "img", "aria-label": "Off", "data-explain": "recording.lamp" });
    const label = h("span", { class: "label", "data-testid": "hub-state", "data-explain": "recording.state" }, "Off");
    const elapsed = h("div", { class: "elapsed", "data-testid": "hub-elapsed", "data-explain": "recording.elapsed" }, "0:00.0");
    const fill = h("div", { class: "fill" });
    const prerollLine = h("span", { "data-testid": "hub-preroll", "data-explain": "recording.preroll" });
    const disk = h("div", { class: "disk", "data-testid": "hub-disk", "data-explain": "recording.hub-disk" }, "Not armed");
    const diskNote = h("div", { class: "disk-note" });
    const takes = h("ul", { class: "takes", "data-testid": "hub-takes", "data-explain": "recording.take" });

    const arm = h("button", { type: "button", class: "main arm", "data-testid": "hub-arm", "data-explain": "recording.arm" }, "Arm");
    const offered = bindArm(host, arm);
    const record = h("button", { type: "button", class: "main record", "data-testid": "hub-record", "data-explain": "recording.record" }, "● Record");
    record.addEventListener("click", () => void model.record());
    const stop = h("button", { type: "button", class: "main stop", "data-testid": "hub-stop", "data-explain": "recording.stop" }, "■ Stop");
    stop.addEventListener("click", () => void model.stop());
    const disarm = h("button", { type: "button", class: "disarm", "data-testid": "hub-disarm", "data-explain": "recording.disarm" }, "Disarm");
    this.onDisconnect(bindConfirm(disarm, "Disarm", () => void model.disarm(true), () => model.status.peek()?.state === "recording"));

    // Only in the desktop app is there a window to put in and out of full screen, or to close.
    const windowed = signal(false);
    const fullScreen = h("button", { type: "button", "data-testid": "hub-full-screen", "data-explain": "recording.hub-full-screen", hidden: !inWindow() }, "Full screen");
    fullScreen.addEventListener("click", () => {
      windowed.value = !windowed.peek();
      toWindow(windowed.peek() ? "windowed" : "full-screen");
    });
    const close = h("button", { type: "button", "data-testid": "hub-close", "data-explain": "recording.hub-close", hidden: !inWindow() }, "Close the hub");
    close.addEventListener("click", () => toWindow("close"));

    const autoText = h("span", { class: "auto", "data-testid": "hub-auto-arm", "data-explain": "recording.auto-arm-state" });
    const autoOff = autoArmOffButton(host, "hub-auto-arm-off");
    const warn = h("span", { class: "warn", role: "status", "data-testid": "hub-warning" });
    const keys = h("span", { class: "keys" }, "Space: Record and Stop · Esc: leave full screen");

    this.root.replaceChildren(
      h("div", { class: "bar" }, preset, time),
      h(
        "div",
        { class: "middle" },
        h("div", { class: "big" }, h("div", { class: "state" }, lamp, label), elapsed, h("div", { class: "preroll" }, h("div", { class: "track meter", role: "img", "aria-label": "Pre-roll held", "data-explain": "recording.preroll" }, fill), prerollLine)),
        h("div", { class: "side" }, h("div", {}, h("h2", { "data-explain": "recording.hub-disk" }, "Disk left"), disk, diskNote), h("div", {}, h("h2", { "data-explain": "recording.takes" }, "Last takes"), takes), h("div", {}, h("h2", { "data-explain": "recording.metronome" }, "Metronome"), metronomeCompact(host, "hub", false))),
      ),
      h("div", { class: "meters" }, channelMeters(host, "hub-channel", "Arm to see every channel's level here.")),
      h("div", { class: "controls" }, arm, record, stop, h("div", { class: "aside" }, disarm, fullScreen, close)),
      h("div", { class: "foot" }, warn, autoText, autoOff, keys),
    );

    spaceToggles(host, true);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (toWindow("windowed")) windowed.value = true;
        else if (document.fullscreenElement !== null) void document.exitFullscreen();
      } else if (event.key === "F11" && inWindow()) {
        event.preventDefault();
        windowed.value = !windowed.peek();
        toWindow(windowed.peek() ? "windowed" : "full-screen");
      }
    };
    document.addEventListener("keydown", onKey);
    this.onDisconnect(() => document.removeEventListener("keydown", onKey));

    this.watch(() => {
      time.textContent = timeOfDay(now.value);
    });
    this.watch(() => {
      fullScreen.textContent = windowed.value ? "Full screen" : "Leave full screen";
    });

    this.watch(() => {
      const status = model.status.value;
      const busy = model.busy.value;
      const connected = store.connected.value;
      void store.workspace.value;
      const state = status?.state ?? "off";
      this.dataset["state"] = state;
      label.textContent = stateLabel(status);
      lamp.setAttribute("aria-label", stateLabel(status));
      elapsed.textContent = clockText(status?.take?.elapsed_seconds);

      const { name, problem } = offered();
      preset.textContent = status?.preset?.name ?? name ?? "No preset yet";
      const off = state === "off";
      fill.style.setProperty("--fill", String(prerollFill(status)));
      prerollLine.textContent = off ? "Arm to start holding a pre-roll." : prerollText(status);

      const left = diskLeftText(status);
      disk.textContent = left ?? (off ? "Not armed" : "Looking");
      diskNote.textContent = left === undefined ? "Arm to see how long the disk would record for." : `of recording at ${status?.rate ?? ""} Hz, ${status?.channels.length ?? 0} channel${status?.channels.length === 1 ? "" : "s"}${status?.disk_low === true ? ". The disk is getting full." : ""}`;

      arm.hidden = !off && state !== "arming";
      arm.disabled = !connected || busy !== undefined || state === "arming" || problem !== undefined;
      const taking = state === "recording" || state === "counting_in";
      record.hidden = !(state === "armed" || taking);
      record.disabled = !connected || busy !== undefined || state !== "armed";
      stop.hidden = !(state === "armed" || taking);
      stop.disabled = !connected || (busy !== undefined && busy !== "metronome") || !taking;
      disarm.hidden = off || state === "arming";
      disarm.disabled = !connected || busy === "disarm" || state === "disarming";

      const said = connected ? warnings(model.problem.value, status) : [];
      if (said.length === 0 && off && problem !== undefined) said.push(problem);
      warn.textContent = said.join(" ");
      warn.hidden = said.length === 0;
      warn.classList.toggle("disconnected", !connected);
      if (!connected) {
        warn.hidden = false;
        warn.textContent = "The server is not connected. The buttons wait for it to come back.";
      }
    });
    this.watch(() => {
      const status = model.status.value;
      const auto = autoArmText(status?.auto_arm, now.value.getTime());
      autoText.hidden = auto === undefined;
      autoText.textContent = auto ?? "";
      autoOff.hidden = auto === undefined || store.phone;
    });
    this.watch(() => {
      const list = model.takes.value.slice(0, HUB_TAKES);
      takes.replaceChildren(...(list.length === 0 ? [h("li", {}, "No takes yet")] : list.map((take) => h("li", { title: take.folder }, takeLine(take)))));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-recording-hub": GaRecordingHub;
  }
}
