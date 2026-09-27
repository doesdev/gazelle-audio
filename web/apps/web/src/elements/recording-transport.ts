// The recorder's transport: the preset, the big state display, the pre-roll, and Arm, Record, Stop
// and Disarm. The Recording page shows it whole; the Remote page shows it compact, at thumb size.
// Both read and press through the one model (`store/recording.ts`), so a press on the phone shows on
// the computer at once, and the other way round.
//
// - **Off**: a grey dot. The preset menu and Arm.
// - **Armed**: a hollow red ring that pulses, the pre-roll filling, and Record. While armed Gazelle
//   holds the audio drivers, which the transport says in words beside it.
// - **Recording**: a solid red dot, "Recording" in red, the time since Record, and Stop.
//
// Record and Stop are one press each: missing the start of a take is the worse failure. Arm is one
// press as well, but its first use in a browser says what it does first. Disarming while a take is
// running asks for a second press.

import { h } from "../core/dom.ts";
import { untracked } from "../core/signal.ts";
import { ARM_EXPLAINED_KEY, clockText, diskText, lossText, PRESET_KEY, prerollFill, prerollText, presetProblem, presetToOffer, recordingModel, stateLabel } from "../store/recording.ts";
import { bindConfirm } from "./controls.ts";
import { useStore } from "./element.ts";

/** What the transport needs of the page it is on. */
export interface TransportHost {
  watch(fn: () => void): void;
  onDisconnect(fn: () => void): void;
}

/** What drivers being held means, said wherever Arm is. The drivers' behaviour with a second program is not known yet. */
export const DRIVER_HOLD =
  "While armed or recording, Gazelle holds the audio drivers of both interfaces. A DAW may not be able to use them until you disarm: whether the Antelope drivers let a second program in at the same time has not been tried yet.";

/** The same, short enough for a phone. */
export const DRIVER_HOLD_SHORT = "While armed, Gazelle holds the audio drivers, and a DAW may not be able to use them until you disarm.";

/** What Arm does, said once in each browser before the first Arm. */
export const ARM_EXPLAINED =
  "Arm opens the interfaces through Gazelle Aggregate and keeps the last stretch of what they hear in memory, so Record can start a take a little before you press it. It reserves the preset's share of the free memory at once, and gives it back when you disarm. Nothing is written to disk until you press Record.";

export const TRANSPORT_STYLES = `
  .transport { display: grid; gap: 10px; padding: 12px; border: 1px solid var(--ga-border-subtle); border-radius: 6px; background: var(--ga-surface-raised); }
  .transport .top { display: flex; flex-wrap: wrap; align-items: center; gap: 10px 16px; }
  .transport .state { display: flex; align-items: center; gap: 12px; min-width: 220px; }
  .transport .lamp { flex: none; width: 26px; height: 26px; border-radius: 50%; border: 3px solid var(--ga-text-muted); box-sizing: border-box; }
  .transport[data-state="armed"] .lamp, .transport[data-state="arming"] .lamp { border-color: var(--ga-notice-error); background: transparent; animation: ga-armed 1.2s ease-in-out infinite; }
  .transport[data-state="recording"] .lamp { border-color: var(--ga-notice-error); background: var(--ga-notice-error); box-shadow: 0 0 12px var(--ga-notice-error); }
  @keyframes ga-armed { 50% { opacity: 0.35; } }
  @media (prefers-reduced-motion: reduce) { .transport .lamp { animation: none !important; } }
  .transport .label { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 30px; font-weight: 700; letter-spacing: 0.02em; }
  .transport[data-state="recording"] .label { color: var(--ga-notice-error); }
  .transport .clock { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 30px; font-variant-numeric: tabular-nums; color: var(--ga-text-secondary); }
  .transport[data-state="recording"] .clock { color: var(--ga-text-primary); }
  .transport .clock[hidden] { display: none; }
  .transport .preset { display: grid; gap: 2px; font-size: 11px; color: var(--ga-text-secondary); }
  .transport .preset select { min-width: 180px; }
  .transport .buttons { display: flex; flex-wrap: wrap; gap: 8px; }
  .transport .buttons button { min-width: 96px; min-height: 40px; font-size: 15px; font-weight: 700; }
  .transport .record { border-color: var(--ga-notice-error); color: var(--ga-notice-error); }
  .transport .record:not(:disabled):hover { background: var(--ga-notice-error); color: var(--ga-text-inverse); }
  .transport .disarm[data-armed] { outline: 2px dashed var(--ga-notice-error); outline-offset: 2px; }
  .transport .preroll { display: grid; gap: 4px; font-size: 12px; color: var(--ga-text-secondary); }
  .transport .preroll .bar { height: 8px; border-radius: 4px; overflow: hidden; background: var(--ga-surface-inset); }
  .transport .preroll .fill { height: 100%; width: calc(var(--fill, 0) * 100%); background: var(--ga-accent); transition: width 180ms linear; }
  .transport[data-state="recording"] .preroll .fill { background: var(--ga-notice-error); }
  .transport .note { margin: 0; font-size: 12px; color: var(--ga-text-muted); }
  .transport .warn { margin: 0; font-size: 12px; color: var(--ga-notice-warning); }
  .transport .warn[hidden], .transport .note[hidden] { display: none; }
  .transport .explain { display: grid; gap: 8px; padding: 10px; border: 1px solid var(--ga-accent); border-radius: 4px; background: var(--ga-surface-inset); font-size: 13px; }
  .transport .explain[hidden] { display: none; }
  .transport .explain .actions { display: flex; gap: 8px; }
  /* compact (the Remote page): thumb-sized, one column. */
  .transport[compact] .label, .transport[compact] .clock { font-size: 24px; }
  .transport[compact] .buttons { display: grid; grid-template-columns: 1fr 1fr; }
  .transport[compact] .buttons button { min-height: 56px; font-size: 16px; }
  .transport[compact] .preset select { width: 100%; min-height: 44px; font-size: 14px; }
`;

function remembered(key: string): string | undefined {
  try {
    return localStorage.getItem(key) ?? undefined;
  } catch {
    return undefined;
  }
}

function remember(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Without storage it is simply asked again next time.
  }
}

/** The transport, following the recorder until the page goes. */
export function recordingTransport(host: TransportHost, compact: boolean): HTMLElement {
  const store = useStore();
  const model = recordingModel(store);
  host.onDisconnect(model.activate());
  const where = compact ? "remote" : "recording";

  const lamp = h("span", { class: "lamp", role: "img", "aria-label": "Off", "data-explain": "recording.lamp" });
  const label = h("span", { class: "label", "data-testid": `${where}-state`, "data-explain": "recording.state" }, "Off");
  const clock = h("span", { class: "clock", "data-testid": `${where}-elapsed`, "data-explain": "recording.elapsed", hidden: true }, "0:00.0");
  const choose = h("select", { "aria-label": "Preset", "data-testid": `${where}-preset`, "data-explain": "recording.preset-select" });
  choose.addEventListener("change", () => remember(PRESET_KEY, choose.value));
  const preset = h("label", { class: "preset" }, "Preset", choose);

  const explain = h("div", { class: "explain", hidden: true, "data-testid": `${where}-arm-explained` });
  const arm = h("button", { type: "button", class: "arm", "data-testid": `${where}-arm`, "data-explain": "recording.arm" }, "Arm");
  const record = h("button", { type: "button", class: "record", "data-testid": `${where}-record`, "data-explain": "recording.record" }, "● Record");
  const stop = h("button", { type: "button", class: "stop", "data-testid": `${where}-stop`, "data-explain": "recording.stop" }, "■ Stop");
  const disarm = h("button", { type: "button", class: "disarm", "data-testid": `${where}-disarm`, "data-explain": "recording.disarm" }, "Disarm");
  const bar = h("div", { class: "bar meter", role: "img", "aria-label": "Pre-roll held", "data-testid": `${where}-preroll-bar`, "data-explain": "recording.preroll" }, h("div", { class: "fill" }));
  const prerollLine = h("span", { "data-testid": `${where}-preroll`, "data-explain": "recording.preroll" });
  const warn = h("p", { class: "warn", role: "status", "data-testid": `${where}-warning`, hidden: true });
  const hold = h("p", { class: "note", "data-testid": `${where}-driver-hold` }, compact ? DRIVER_HOLD_SHORT : DRIVER_HOLD);

  const doArm = () => {
    const id = choose.value;
    if (id !== "") void model.arm(id);
  };
  arm.addEventListener("click", () => {
    // Explained once in each browser, then one press.
    if (remembered(ARM_EXPLAINED_KEY) === undefined) {
      explain.hidden = false;
      return;
    }
    doArm();
  });
  const armNow = h("button", { type: "button", "data-testid": `${where}-arm-explained-go`, "data-explain": "recording.arm-explained-go" }, "Arm");
  const notNow = h("button", { type: "button", "data-testid": `${where}-arm-explained-cancel`, "data-explain": "recording.arm-explained-cancel" }, "Not now");
  armNow.addEventListener("click", () => {
    remember(ARM_EXPLAINED_KEY, "1");
    explain.hidden = true;
    doArm();
  });
  notNow.addEventListener("click", () => {
    explain.hidden = true;
  });
  explain.append(h("p", { style: "margin: 0" }, ARM_EXPLAINED), h("p", { style: "margin: 0" }, DRIVER_HOLD), h("div", { class: "actions" }, armNow, notNow));

  record.addEventListener("click", () => void model.record());
  stop.addEventListener("click", () => void model.stop());
  // A take being recorded asks first; armed and not recording, one press.
  host.onDisconnect(bindConfirm(disarm, "Disarm", () => void model.disarm(true), () => model.status.peek()?.state === "recording"));

  const element = h(
    "div",
    { class: "transport", "data-testid": `${where}-transport`, compact: compact || undefined },
    h("div", { class: "top" }, h("div", { class: "state" }, lamp, label, clock), preset),
    h("div", { class: "buttons" }, arm, record, stop, disarm),
    explain,
    h("div", { class: "preroll" }, bar, prerollLine),
    warn,
    hold,
  );

  host.watch(() => {
    const presets = store.workspace.value?.recording?.presets ?? [];
    const status = model.status.value;
    const shown = untracked(() => choose.value);
    const offer = presets.some((p) => p.id === shown) ? shown : presetToOffer(presets, status, remembered(PRESET_KEY));
    choose.replaceChildren(...presets.map((p) => h("option", { value: p.id }, p.name)));
    if (presets.length === 0) choose.append(h("option", { value: "" }, "No presets yet"));
    choose.value = status?.preset?.id ?? offer ?? "";
  });

  host.watch(() => {
    const status = model.status.value;
    const busy = model.busy.value;
    const connected = store.connected.value;
    const state = status?.state ?? "off";
    element.dataset["state"] = state;
    label.textContent = stateLabel(status);
    lamp.setAttribute("aria-label", stateLabel(status));
    clock.hidden = state !== "recording";
    clock.textContent = clockText(status?.take?.elapsed_seconds);
    const presets = untracked(() => store.workspace.value?.recording?.presets ?? []);
    const chosen = presets.find((p) => p.id === choose.value);
    const off = state === "off";
    choose.disabled = !off || !connected || busy !== undefined;
    arm.hidden = !off && state !== "arming";
    arm.disabled = !connected || busy !== undefined || state === "arming" || presetProblem(chosen) !== undefined;
    arm.title = presetProblem(chosen) ?? "Open the interfaces and start holding a pre-roll";
    record.disabled = !connected || state !== "armed";
    stop.disabled = !connected || state !== "recording";
    // At thumb size only what can be pressed now is shown, so the one that matters is the big one.
    record.hidden = compact && state !== "armed";
    stop.hidden = compact && state !== "recording";
    disarm.hidden = off || state === "arming";
    disarm.disabled = !connected || busy === "disarm" || state === "disarming";
    bar.style.setProperty("--fill", String(prerollFill(status)));
    prerollLine.textContent = off ? "Arm to start holding a pre-roll." : prerollText(status);
    const said = [model.problem.value, status?.problem, lossText(status), status?.disk_low ? `The disk is getting full: ${diskText(status)}.` : undefined, status?.reset_asked ? "A driver asked to be restarted. Disarm and arm again when you can: the take so far is safe." : undefined].filter((s): s is string => s !== undefined && s !== "");
    warn.hidden = said.length === 0;
    warn.textContent = said.join(" ");
  });
  return element;
}
