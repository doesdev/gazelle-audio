// The metronome's controls, in three sizes: the whole of it for the Recording page's Metronome
// section, start and stop with the tempo and a beat light for the hub and the phone, and a single
// button with its tempo and a beat dot for the widget. All of them read and press through the one
// recording model (`store/recording.ts`), whose state carries the metronome's.
//
// Starting it asks nothing, except the first time in each browser, when it says which outputs it
// plays to and that it holds the drivers, as arming does. Stopping it is always one press.
// A phone may start and stop it and change its tempo and volume; everything else is shown to a phone
// and changed on the computer.

import { h } from "../core/dom.ts";
import { untracked } from "../core/signal.ts";
import {
  beatAt,
  clampOffset,
  clampTempo,
  COUNT_IN_MAX,
  DENOMINATORS,
  listText,
  METRONOME_EXPLAINED_KEY,
  METRONOME_HOLDS,
  METRONOME_NOT_IN_TAKES,
  metronomeState,
  NUMERATOR_MAX,
  OFFSET_MS_MAX,
  SOUNDS,
  SUBDIVISIONS,
  TapTempo,
  tempoText,
  VOLUME_MAX,
  VOLUME_MIN,
  volumeText,
  type MetronomeSettings,
  type MetronomeStatus,
} from "../store/metronome.ts";
import { recordingModel } from "../store/recording.ts";
import { bindConfirm } from "./controls.ts";
import { commitOnEnter, useStore } from "./element.ts";
import { remember, remembered, type TransportHost } from "./recording-transport.ts";

export const METRONOME_STYLES = `
  .metronome { display: grid; gap: 10px; }
  .metronome .head { display: flex; flex-wrap: wrap; align-items: center; gap: 10px 16px; }
  .metronome .start { min-width: 96px; min-height: 40px; font-size: 15px; font-weight: 700; }
  .start[aria-pressed="true"] { border-color: var(--ga-accent); background: var(--ga-accent); color: var(--ga-accent-text); }
  .metronome .said { font-size: 13px; color: var(--ga-text-secondary); }
  .metronome .m-fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(210px, 1fr)); gap: 8px 12px; }
  .metronome .m-fields > label, .metronome .m-fields > .field { display: grid; gap: 2px; font-size: 11px; color: var(--ga-text-secondary); }
  .metronome .m-fields > label > select, .metronome .m-fields input[type="range"] { width: 100%; box-sizing: border-box; }
  .metronome .signature select { width: auto; }
  .metronome .check { display: flex; align-items: center; gap: 6px; font-size: 13px; color: var(--ga-text-primary); }
  .metronome .note { margin: 0; font-size: 12px; color: var(--ga-text-muted); }
  .metronome .note[hidden], .metronome .explain[hidden], .metronome .warn[hidden] { display: none; }
  .metronome .warn { margin: 0; font-size: 12px; color: var(--ga-notice-warning); }
  .metronome .explain { display: grid; gap: 8px; padding: 10px; border: 1px solid var(--ga-accent); border-radius: 4px; background: var(--ga-surface-inset); font-size: 13px; }
  .metronome .explain .actions { display: flex; gap: 8px; }
  .metronome .outputs { display: grid; gap: 8px; }
  .metronome .interface { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 2px 12px; align-content: start; padding: 6px 8px; border: 1px solid var(--ga-border-subtle); border-radius: 4px; }
  .metronome .interface h3 { grid-column: 1 / -1; margin: 0 0 4px; font-size: 13px; }
  .metronome .interface label { display: flex; align-items: center; gap: 6px; font-size: 12px; }
  .tempo { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; min-width: 0; }
  .tempo input { flex: none; width: 5.5em; box-sizing: border-box; text-align: right; font-variant-numeric: tabular-nums; }
  .tempo .unit { font-size: 12px; color: var(--ga-text-secondary); }
  .tempo button { min-width: 32px; }
  .beats { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
  .beats .dot { width: 12px; height: 12px; border-radius: 50%; border: 2px solid var(--ga-text-muted); box-sizing: border-box; }
  .beats .dot.one { width: 16px; height: 16px; }
  .beats .dot[data-on] { border-color: var(--ga-accent); background: var(--ga-accent); }
  .beats .dot.one[data-on] { border-color: var(--ga-notice-error); background: var(--ga-notice-error); }
  .beats[data-counting] .dot[data-on] { border-color: var(--ga-notice-warning); background: var(--ga-notice-warning); }
`;

/** The model's metronome, and its settings. */
function metronomeOf(store = useStore()): { status: MetronomeStatus | undefined; settings: MetronomeSettings | undefined } {
  const status = recordingModel(store).status.value?.metronome;
  return { status, settings: status?.settings };
}

/**
 * Beat lights, one per beat of the bar, the downbeat larger, lit from the click's own position and
 * moved between frames by the page's clock. They follow the bar's length as it changes.
 */
export function beatLights(host: TransportHost, testid: string, most = 16): HTMLElement {
  const store = useStore();
  const box = h("div", { class: "beats", role: "img", "aria-label": "Beat", "data-testid": testid, "data-explain": "metronome.beats" });
  let dots: HTMLElement[] = [];
  let frame = 0;
  const light = () => {
    const status = untracked(() => recordingModel(store).status.peek()?.metronome);
    const at = beatAt(status, Date.now());
    // One light flashes on every beat; several show which beat of the bar it is.
    dots.forEach((dot, index) => dot.toggleAttribute("data-on", at !== undefined && (dots.length === 1 ? at.into < 0.25 : index === at.beat - 1)));
    box.setAttribute("aria-label", at === undefined ? "Beat: stopped" : `Beat ${at.beat} of ${status?.beats_per_bar ?? 0}`);
    frame = at === undefined ? 0 : requestAnimationFrame(light);
  };
  host.watch(() => {
    const { status, settings } = metronomeOf(store);
    const count = Math.min(most, Math.max(1, status?.running === true ? status.beats_per_bar : (settings?.numerator ?? 4)));
    if (dots.length !== count) {
      dots = Array.from({ length: count }, (_, index) => h("span", { class: index === 0 ? "dot one" : "dot" }));
      box.replaceChildren(...dots);
    }
    box.toggleAttribute("data-counting", status?.count_in !== undefined);
    cancelAnimationFrame(frame);
    light();
  });
  host.onDisconnect(() => cancelAnimationFrame(frame));
  return box;
}

/** The names of the outputs the metronome plays to: the open session's own, else the settings', named as the Aggregate page names them. */
function outputNames(status: MetronomeStatus | undefined): string[] {
  const store = useStore();
  if (status?.open === true && status.outputs.length > 0) return status.outputs.map((c) => c.name);
  const { inputs: outputs } = untracked(() => store.aggregateInputs(false));
  return (status?.settings.outputs ?? []).map((pick) => outputs.find((c) => c.index === pick.device && c.channel === pick.channel)?.text ?? `Output ${pick.channel + 1} of interface ${pick.device + 1}`);
}

/** What the first Start in a browser says. */
function explanation(status: MetronomeStatus | undefined): string {
  const names = outputNames(status);
  return `${names.length === 0 ? "No outputs are chosen yet, so it has nowhere to play." : `It plays to ${listText(names)}.`} ${METRONOME_HOLDS} ${METRONOME_NOT_IN_TAKES}`;
}

/**
 * Start and Stop, one button. With `panel`, the first Start in a browser opens it with the
 * explanation; without one (the hub, the widget), the first press reads Confirm and the button's
 * title says it all.
 */
export function startStop(host: TransportHost, testid: string, panel?: { box: HTMLElement; text: HTMLElement; go: HTMLButtonElement }): HTMLButtonElement {
  const store = useStore();
  const model = recordingModel(store);
  const button = h("button", { type: "button", class: "start", "aria-pressed": "false", "data-testid": testid, "data-explain": "metronome.start" }, "Start");
  const running = () => untracked(() => model.status.peek()?.metronome?.running === true);
  const explained = () => remembered(METRONOME_EXPLAINED_KEY) !== undefined;
  const start = () => {
    remember(METRONOME_EXPLAINED_KEY, "1");
    void model.metronome("start");
  };
  if (panel === undefined) {
    host.onDisconnect(bindConfirm(button, () => (running() ? "Stop" : "Start"), () => (running() ? void model.metronome("stop") : start()), () => !running() && !explained()));
  } else {
    button.addEventListener("click", () => {
      if (running()) void model.metronome("stop");
      else if (explained()) start();
      else {
        panel.text.textContent = explanation(untracked(() => model.status.peek()?.metronome));
        panel.box.hidden = false;
      }
    });
    panel.go.addEventListener("click", () => {
      panel.box.hidden = true;
      start();
    });
  }
  host.watch(() => {
    const status = model.status.value?.metronome;
    const on = status?.running === true;
    button.setAttribute("aria-pressed", String(on));
    if (!button.hasAttribute("data-armed")) button.textContent = on ? "Stop" : "Start";
    button.disabled = !store.connected.value || model.busy.value === "metronome";
    button.title = on ? "Stop the metronome" : explanation(status);
  });
  return button;
}

/** The tempo: its value, a step down and up, and with `tap`, tap tempo. */
export function tempoControls(host: TransportHost, where: string, tap: boolean): HTMLElement {
  const store = useStore();
  const model = recordingModel(store);
  const current = () => untracked(() => model.status.peek()?.metronome?.settings);
  const input = h("input", { type: "number", min: 20, max: 400, step: 0.1, inputmode: "decimal", "aria-label": "Tempo in BPM", "data-testid": `${where}-tempo`, "data-explain": "metronome.tempo" });
  commitOnEnter(input, (value) => void model.setMetronome({ tempo: clampTempo(Number(value)) }), () => String(current()?.tempo ?? ""));
  const step = (by: number, label: string, testid: string) => {
    const button = h("button", { type: "button", "aria-label": `${label} 1 BPM`, "data-testid": `${where}-${testid}`, "data-explain": "metronome.tempo-step" }, label);
    button.addEventListener("click", () => void model.setMetronome({ tempo: clampTempo((current()?.tempo ?? 120) + by) }));
    return button;
  };
  const down = step(-1, "−", "slower");
  const up = step(1, "+", "faster");
  const taps = new TapTempo();
  const tapButton = tap ? h("button", { type: "button", class: "tap", "data-testid": `${where}-tap`, "data-explain": "metronome.tap" }, "Tap") : undefined;
  tapButton?.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    const tempo = taps.tap(performance.now(), current()?.denominator ?? 4);
    if (tempo !== undefined) void model.setMetronome({ tempo });
  });
  tapButton?.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    const tempo = taps.tap(performance.now(), current()?.denominator ?? 4);
    if (tempo !== undefined) void model.setMetronome({ tempo });
  });
  host.watch(() => {
    const settings = model.status.value?.metronome?.settings;
    const connected = store.connected.value;
    if (settings !== undefined && input.ownerDocument.activeElement !== input && (input.getRootNode() as ShadowRoot | Document).activeElement !== input) input.value = tempoText(settings.tempo);
    for (const control of [input, down, up, tapButton]) if (control !== undefined) control.disabled = !connected || settings === undefined;
  });
  return h("span", { class: "tempo" }, down, input, up, h("span", { class: "unit" }, "BPM"), tapButton ?? false);
}

/** The volume: a slider from -60 to -6 dBFS, and its value. */
export function volumeControl(host: TransportHost, where: string): HTMLElement {
  const store = useStore();
  const model = recordingModel(store);
  const slider = h("input", { type: "range", min: VOLUME_MIN, max: VOLUME_MAX, step: 1, "aria-label": "Metronome volume", "data-testid": `${where}-volume`, "data-explain": "metronome.volume" });
  const readout = h("span", { class: "readout", "data-explain": "metronome.volume" });
  slider.addEventListener("input", () => (readout.textContent = volumeText(Number(slider.value))));
  slider.addEventListener("change", () => void model.setMetronome({ volume_db: Number(slider.value) }));
  host.watch(() => {
    const settings = model.status.value?.metronome?.settings;
    if (settings !== undefined && (slider.getRootNode() as ShadowRoot | Document).activeElement !== slider) slider.value = String(settings.volume_db);
    readout.textContent = settings === undefined ? "" : volumeText(settings.volume_db);
    slider.disabled = !store.connected.value || settings === undefined;
  });
  return h("label", {}, h("span", {}, "Volume ", readout), slider);
}

/** **The Metronome section** of the Recording page: everything. */
export function metronomeSection(host: TransportHost): HTMLElement {
  const store = useStore();
  const model = recordingModel(store);
  const phone = store.phone;
  const set = (change: Partial<MetronomeSettings>) => void model.setMetronome(change);

  const text = h("p", { style: "margin: 0" });
  const go = h("button", { type: "button", "data-testid": "metronome-explained-go", "data-explain": "metronome.explained-go" }, "Start");
  const notNow = h("button", { type: "button", "data-testid": "metronome-explained-cancel", "data-explain": "metronome.explained-cancel" }, "Not now");
  const box = h("div", { class: "explain", hidden: true, "data-testid": "metronome-explained" }, text, h("div", { class: "actions" }, go, notNow));
  notNow.addEventListener("click", () => (box.hidden = true));
  const start = startStop(host, "metronome-start", { box, text, go });
  const state = h("span", { class: "said", "data-testid": "metronome-state", "data-explain": "metronome.state" });
  const preview = h("button", { type: "button", "data-testid": "metronome-preview", "data-explain": "metronome.preview" }, "Preview a bar");
  preview.addEventListener("click", () => void model.metronome("preview"));

  const numerator = h("select", { "aria-label": "Beats in a bar", "data-testid": "metronome-numerator", "data-explain": "metronome.signature" }, ...Array.from({ length: NUMERATOR_MAX }, (_, i) => h("option", { value: String(i + 1) }, String(i + 1))));
  numerator.addEventListener("change", () => set({ numerator: Number(numerator.value) }));
  const denominator = h("select", { "aria-label": "The note a beat is", "data-testid": "metronome-denominator", "data-explain": "metronome.signature" }, ...DENOMINATORS.map((d) => h("option", { value: String(d) }, String(d))));
  denominator.addEventListener("change", () => set({ denominator: Number(denominator.value) }));
  const accent = h("input", { type: "checkbox", "data-testid": "metronome-accent", "data-explain": "metronome.accent" });
  accent.addEventListener("change", () => set({ accent: accent.checked }));
  const subdivision = h("select", { "data-testid": "metronome-subdivision", "data-explain": "metronome.subdivision" }, ...SUBDIVISIONS.map(([value, words]) => h("option", { value }, words)));
  subdivision.addEventListener("change", () => set({ subdivision: subdivision.value as MetronomeSettings["subdivision"] }));
  const sound = h("select", { "data-testid": "metronome-sound", "data-explain": "metronome.sound" }, ...SOUNDS.map(([value, words]) => h("option", { value }, words)));
  sound.addEventListener("change", () => set({ sound: sound.value as MetronomeSettings["sound"] }));
  const countIn = h("select", { "data-testid": "metronome-count-in", "data-explain": "metronome.count-in" }, ...Array.from({ length: COUNT_IN_MAX + 1 }, (_, n) => h("option", { value: String(n) }, n === 0 ? "None" : n === 1 ? "1 bar" : `${n} bars`)));
  countIn.addEventListener("change", () => set({ count_in_bars: Number(countIn.value) }));
  const follow = h("input", { type: "checkbox", "data-testid": "metronome-follow", "data-explain": "metronome.follow" });
  follow.addEventListener("change", () => set({ follow_record: follow.checked }));
  const offset = h("input", { type: "number", min: -OFFSET_MS_MAX, max: OFFSET_MS_MAX, step: 0.01, inputmode: "decimal", "aria-label": "Latency offset in ms", "data-testid": "metronome-offset", "data-explain": "metronome.offset" });
  commitOnEnter(offset, (value) => set({ latency_offset_ms: clampOffset(Number(value)) }), () => String(untracked(() => model.status.peek()?.metronome?.settings.latency_offset_ms) ?? ""));

  const outputs = h("div", { class: "outputs", "data-testid": "metronome-outputs" });
  const fixed = h("p", { class: "note", "data-testid": "metronome-outputs-fixed", hidden: true }, "The outputs are fixed while the interfaces are open. Stop the metronome and disarm to change them.");
  const outputsProblem = h("p", { class: "warn", role: "status", "data-testid": "metronome-outputs-problem", hidden: true });
  const playsTo = h("p", { class: "note", "data-testid": "metronome-plays-to" });

  host.watch(() => {
    const { status, settings } = metronomeOf(store);
    const connected = store.connected.value;
    const recorder = model.status.value?.state ?? "off";
    state.textContent = metronomeState(status);
    preview.disabled = !connected || recorder === "off" || status?.running === true || (settings?.outputs.length ?? 0) === 0;
    preview.title = recorder === "off" ? "A preview plays only while armed, so it never opens the interfaces by itself." : "One bar, 12 dB under the volume.";
    if (settings !== undefined) {
      numerator.value = String(settings.numerator);
      denominator.value = String(settings.denominator);
      accent.checked = settings.accent;
      subdivision.value = settings.subdivision;
      sound.value = settings.sound;
      countIn.value = String(settings.count_in_bars);
      follow.checked = settings.follow_record;
      if ((offset.getRootNode() as ShadowRoot | Document).activeElement !== offset) offset.value = String(settings.latency_offset_ms);
    }
    for (const control of [numerator, denominator, accent, subdivision, sound, countIn, follow, offset]) control.disabled = !connected || phone || settings === undefined;
    const names = outputNames(status);
    playsTo.textContent = names.length === 0 ? "Choose where it plays: tick an output, or a pair such as the two your headphones are on." : `Plays to ${listText(names)}. ${METRONOME_NOT_IN_TAKES}`;
    outputsProblem.hidden = status?.outputs_problem === undefined;
    outputsProblem.textContent = status?.outputs_problem === undefined ? "" : `Not played to: ${status.outputs_problem}.`;
    fixed.hidden = status?.open !== true || phone;
  });

  // The output picker, rebuilt only when what it shows changes.
  let built = "";
  host.watch(() => {
    const { status, settings } = metronomeOf(store);
    const list = store.aggregateInputs(false);
    const locked = phone || status?.open === true || !store.connected.value || settings === undefined;
    const chosen = settings?.outputs ?? [];
    const key = JSON.stringify([list, chosen, locked]);
    if (key === built) return;
    built = key;
    untracked(() => {
      outputs.replaceChildren();
      if (list.inputs.length === 0) {
        outputs.append(h("p", { class: "note" }, "The aggregate's interfaces are set up on the Aggregate page; their outputs are listed here once it knows them."));
        return;
      }
      list.devices.forEach((device, index) => {
        const own = list.inputs.filter((c) => c.index === index);
        if (own.length === 0) return;
        const boxes = own.map((c) => {
          const on = chosen.some((pick) => pick.device === index && pick.channel === c.channel);
          const tick = h("input", { type: "checkbox", checked: on, disabled: locked, "data-testid": `metronome-output-${index}-${c.channel}`, "data-explain": "metronome.output", "data-explain-name": c.text });
          tick.addEventListener("change", () => {
            const rest = chosen.filter((pick) => !(pick.device === index && pick.channel === c.channel));
            set({ outputs: tick.checked ? [...rest, { device: index, channel: c.channel }].sort((a, b) => a.device - b.device || a.channel - b.channel) : rest });
          });
          return h("label", {}, tick, c.text);
        });
        outputs.append(h("div", { class: "interface" }, h("h3", {}, device), ...boxes));
      });
    });
  });

  const field = (label: string, ...control: (HTMLElement | string)[]) => h("label", {}, label, ...control);
  return h(
    "div",
    { class: "metronome", "data-testid": "metronome" },
    h("div", { class: "head" }, start, beatLights(host, "metronome-beats"), state, preview),
    box,
    h(
      "div",
      { class: "m-fields" },
      h("div", { class: "field" }, "Tempo (quarter notes a minute)", tempoControls(host, "metronome", true)),
      h("label", {}, "Time signature", h("span", { class: "tempo signature" }, numerator, "/", denominator)),
      field("Sound", sound),
      field("Clicks between the beats", subdivision),
      volumeControl(host, "metronome"),
      field("Count-in before a take", countIn),
      h("label", { class: "field" }, "Latency offset for the Downbeat", h("span", { class: "tempo" }, offset, h("span", { class: "unit" }, "ms"))),
    ),
    h("div", { class: "row" }, h("label", { class: "check" }, accent, "Accent the first beat of the bar"), h("label", { class: "check" }, follow, "Follows Record: plays whenever a take is recording, and stops with it")),
    h("p", { class: "note" }, "Beats follow the time signature: 6/8 clicks eighth notes, as a DAW does, and the tempo counts quarter notes. With a count-in, Record starts the click if it is not playing, counts the bars and starts the take on the downbeat after them, reaching back into the pre-roll as any take does; the downbeat is marked in the take where a performance in time with the click you hear lands: the round trip the interfaces report after the click, plus the latency offset. Stop during the count-in starts no take."),
    h("p", { class: "note" }, `${METRONOME_HOLDS} The volume is the loudest click's peak, the downbeat's when it is accented; Gazelle never plays it louder than -6 dBFS, whatever is asked.`),
    phone ? h("p", { class: "note" }, "From a phone: start, stop, tempo and volume. The rest is changed on the computer.") : false,
    h("h3", { style: "margin: 4px 0 0; font-size: 13px" }, "Outputs"),
    playsTo,
    outputsProblem,
    fixed,
    outputs,
  );
}

/** **The hub's and the phone's metronome**: Start and Stop, the tempo with Tap, the beat, and with `volume`, the volume. */
export function metronomeCompact(host: TransportHost, where: string, volume: boolean): HTMLElement {
  const store = useStore();
  const model = recordingModel(store);
  const state = h("span", { class: "said", "data-testid": `${where}-metronome-state`, "data-explain": "metronome.state" });
  const line = h("span", { class: "said", "data-testid": `${where}-metronome-line`, "data-explain": "metronome.tempo" });
  host.watch(() => {
    const { status, settings } = metronomeOf(store);
    state.textContent = metronomeState(status);
    line.textContent = settings === undefined ? "" : `${settings.numerator}/${settings.denominator}, ${SOUNDS.find(([value]) => value === settings.sound)?.[1] ?? ""}`;
    void model.busy.value;
  });
  return h(
    "div",
    { class: "metronome", "data-testid": `${where}-metronome` },
    h("div", { class: "head" }, startStop(host, `${where}-metronome-start`), beatLights(host, `${where}-metronome-beats`), state),
    h("div", { class: "head" }, tempoControls(host, `${where}-metronome`, true), line),
    volume ? h("div", { class: "m-fields" }, volumeControl(host, `${where}-metronome`)) : false,
  );
}

/** **The widget's metronome**: one small button, its tempo on it, and a beat dot. */
export function metronomeMini(host: TransportHost): HTMLElement {
  const store = useStore();
  const button = startStop(host, "widget-metronome");
  const label = () => {
    const settings = untracked(() => recordingModel(store).status.peek()?.metronome?.settings);
    return settings === undefined ? "Click" : `♩ ${tempoText(settings.tempo)}`;
  };
  host.watch(() => {
    const status = recordingModel(store).status.value?.metronome;
    if (!button.hasAttribute("data-armed")) button.textContent = label();
    button.setAttribute("aria-label", status?.running === true ? "Stop the metronome" : "Start the metronome");
  });
  return h("span", { class: "mini" }, button, beatLights(host, "widget-metronome-beat", 1));
}
