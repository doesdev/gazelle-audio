// <ga-aggregate>: the Aggregate page. One audio driver a DAW opens, with two or more interfaces
// underneath it, and everything about whether this PC can actually run it.
//
// The page is read from top to bottom and answers four questions in that order. Is it usable, and
// if not, why, in words with a button beside each thing Gazelle can put right? Is the driver
// registered, and is the copy Windows has registered the one that is here now? What is in the
// aggregate, what is each interface doing, and how do you change any of it? And, while a DAW has
// it open, is the clock link holding, which is the gap in samples between each device and the one
// driving the callback.
//
// Almost nothing here decides anything. `GET /api/v1/aggregate` answers all of it at once, reasons
// and fixes included, and the store polls it while this page is on screen: every second while
// audio is running, more slowly while it is not, and not at all once the page has gone. The
// ordinary case is that no DAW has the driver open and the driver has published nothing, which is
// not a fault and does not read as one.
//
// The setup itself lives in the workspace, so editing it here saves it there; the server exports
// the file the driver reads and tells the driver to look again. That includes each interface's
// channels: which of them the aggregate exposes, and what a DAW calls each one.
//
// Which of Gazelle's devices an entry is comes from the answer, not from this page: the server
// resolves it the same way for every route, so an interface nobody has pinned still reads its
// clock, rate and buffer. Choosing one in the menu pins it, and Work it out gives it back.

import { h } from "../core/dom.ts";
import { effect } from "../core/signal.ts";
import {
  aggregateChannels,
  appliedTrimsText,
  autoChannelName,
  buffersMatch,
  cablingSteps,
  calibrateDevices,
  calibrateProblem,
  calibrateProgress,
  calibrateRequest,
  calibrateRunning,
  calibrateStepText,
  channelCounts,
  channelLabel,
  channelsOf,
  channelSummary,
  CHANNEL_LABEL_MAX,
  CLICKS,
  deviceName,
  deviceViews,
  driftFound,
  fixNeedsConfirming,
  gapView,
  isExposed,
  LEVELS_DBFS,
  matchedBy,
  matchNote,
  matchTarget,
  outcomeSummary,
  readingView,
  reconcilePicks,
  resolvedDeviceId,
  slotDevice,
  statusLine,
  suggestedChannelName,
  trimRows,
  trimsToApply,
  viewFor,
  withChannelExposed,
  withChannelName,
  withMeasuredTrims,
  type Aggregate,
  type AggregateAnswer,
  type AggregateCalibrateDirection,
  type AggregateChannelNames,
  type AggregateDevice,
  type AggregateDeviceView,
  type AggregateFix,
  type AggregateReason,
  type CalibratePicks,
} from "../store/aggregate.ts";
import { driverControls } from "../store/driver.ts";
import { SAMPLE_RATES, type Store } from "../store/store.ts";
import { bindConfirm, confirmedChoice } from "./controls.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";

/** `SAMPLE_RATES` in Hz, which is how the aggregate's setup and the driver's file say a rate. */
const RATE_HZ = [32000, 44100, 48000, 88200, 96000, 176400, 192000];

/** The sizes offered when no driver has been read yet, as every Antelope driver here offers. */
const BUFFER_SIZES = [16, 32, 64, 128, 256, 512, 1024, 2048];

const RESTARTS = "every program using these drivers restarts its audio";

export class GaAggregate extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; max-width: 760px; }
      ga-section + ga-section { margin-top: 10px; }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-bottom: 10px; }
      .bar .spacer { flex: 1; }
      .verdict { padding: 2px 10px; border-radius: 3px; font-weight: 700; letter-spacing: 0.04em; background: var(--ga-surface-inset); color: var(--ga-text-muted); }
      .verdict[data-ready="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .verdict[data-ready="false"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .note { margin: 6px 0 0; font-size: 11px; color: var(--ga-text-muted); }
      .note.warning { color: var(--ga-state-mute); }
      code { font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; word-break: break-all; }
      .reasons { display: grid; gap: 6px; margin: 0; padding: 4px 0 0; list-style: none; }
      .reason { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 3px; background: var(--ga-surface-raised); }
      .severity { padding: 0 5px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-inverse); background: var(--ga-text-muted); }
      .severity[data-severity="blocking"] { background: var(--ga-state-mute); }
      .severity[data-severity="warning"] { background: var(--ga-state-solo); }
      .device-card { display: grid; gap: 8px; padding: 8px 10px; border-radius: 3px; background: var(--ga-surface-raised); }
      .device-card + .device-card { margin-top: 6px; }
      .device-head { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
      .device-head .name { width: 160px; }
      .device-head .spacer { flex: 1; }
      .device-head .order { min-width: 26px; padding: 0 4px; }
      .device-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap: 6px 12px; }
      .cell { display: grid; grid-template-columns: max-content minmax(0, 1fr); align-items: center; gap: 8px; }
      .cell .label { color: var(--ga-text-secondary); font-size: 11px; }
      /* A DLL path and a refusal are long, and a phone is narrow: they wrap rather than push the
         page sideways, which is the rule every other page keeps. */
      .fields { grid-template-columns: max-content minmax(0, 1fr); }
      .fields dd, .cell > span, .live-row > * { min-width: 0; }
      .readout { max-width: 100%; overflow-wrap: anywhere; }
      .choice { display: inline-flex; flex-wrap: wrap; align-items: center; gap: 6px; }
      .worked { font-size: 11px; color: var(--ga-text-muted); }
      /* The Channels part of a card: closed to one line, and a row per channel when it is open. */
      .channels { padding: 2px 8px; border-radius: 3px; background: var(--ga-surface-inset); }
      .channels > summary { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 4px 0; cursor: pointer; }
      .channels .side { margin: 8px 0 2px; font-size: 11px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-secondary); }
      .channel-row { display: grid; grid-template-columns: max-content minmax(0, 1fr) minmax(0, 1.4fr) minmax(0, 1fr); align-items: center; gap: 8px; padding: 2px 0; }
      .channel-row > * { min-width: 0; }
      .channel-row .auto, .channel-row .hint { font-size: 11px; overflow-wrap: anywhere; }
      .channel-row .hint { color: var(--ga-text-muted); }
      .channel-row input { min-height: 24px; }
      .expose { min-width: 34px; }
      .expose[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .safe[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .lock { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-muted); background: var(--ga-surface-inset); }
      .lock[data-locked] { color: var(--ga-text-inverse); background: var(--ga-accent); }
      .gap[data-tone="good"] { color: var(--ga-accent); }
      .gap[data-tone="off"], .gap[data-tone="stalled"] { color: var(--ga-state-mute); font-weight: 700; }
      .gap[data-tone="idle"] { color: var(--ga-text-muted); }
      .setup { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 6px 12px; align-items: center; padding: 4px 0; }
      .setup .label { color: var(--ga-text-secondary); font-size: 11px; }
      .setup select, .setup input { justify-self: start; min-height: 24px; }
      .trim { width: 84px; }
      .add { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin-top: 8px; }
      .events { display: grid; gap: 2px; margin: 0; padding: 4px 0 0; list-style: none; font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      .events li { display: flex; gap: 8px; }
      .events .at { color: var(--ga-text-muted); white-space: nowrap; }
      .events .kind { min-width: 110px; color: var(--ga-text-secondary); }
      /* Lining the interfaces up: the pickers, what to patch, the run, and what it measured. */
      .calibrate-row { display: grid; grid-template-columns: minmax(0, 1fr) repeat(2, minmax(0, 1.2fr)); align-items: center; gap: 8px; padding: 3px 0; }
      .calibrate-row > * { min-width: 0; }
      .calibrate-row .who { font-weight: 700; }
      .cables { display: grid; gap: 2px; margin: 0; padding: 4px 0 0; list-style: none; }
      .cables li { display: flex; gap: 8px; align-items: baseline; }
      .cables .at { min-width: 18px; color: var(--ga-text-muted); }
      .cables .cable { font-weight: 700; overflow-wrap: anywhere; }
      .track { flex: 1; min-width: 80px; height: 6px; border-radius: 3px; background: var(--ga-surface-inset); overflow: hidden; }
      .track .fill { height: 100%; background: var(--ga-accent); transition: width 120ms linear; }
      .running { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-top: 6px; }
      .reading { display: grid; grid-template-columns: minmax(0, 1fr) repeat(3, minmax(0, auto)); align-items: center; gap: 10px; padding: 4px 8px; border-radius: 3px; background: var(--ga-surface-raised); }
      .reading + .reading, .trim-row + .trim-row { margin-top: 4px; }
      .reading > * { min-width: 0; }
      .lag[data-tone="good"] { color: var(--ga-accent); }
      .lag[data-tone="off"] { color: var(--ga-state-solo); font-weight: 700; }
      .lag[data-tone="drift"] { color: var(--ga-state-mute); font-weight: 700; }
      .drift { margin: 2px 0 6px; font-size: 11px; color: var(--ga-state-mute); font-weight: 700; }
      .trim-row { display: grid; grid-template-columns: minmax(0, 1fr) repeat(3, minmax(0, auto)); align-items: center; gap: 10px; padding: 4px 8px; border-radius: 3px; background: var(--ga-surface-inset); }
      .trim-row > * { min-width: 0; }
      .confirm, button[data-armed] { outline: 2px dashed var(--ga-state-mute); outline-offset: -2px; }
      .live { display: grid; gap: 6px; }
      .live-row { display: grid; grid-template-columns: minmax(0, 1fr) repeat(4, minmax(0, auto)); align-items: center; gap: 10px; padding: 4px 8px; border-radius: 3px; background: var(--ga-surface-raised); }

      /* Phone width: nothing sits side by side, and the long readouts wrap rather than scroll. */
      @media (max-width: 480px) {
        .reason { grid-template-columns: auto minmax(0, 1fr); }
        .reason .fix { grid-column: 1 / -1; justify-self: start; }
        .device-grid { grid-template-columns: minmax(0, 1fr); }
        .device-head .name { width: 100%; }
        .setup { grid-template-columns: minmax(0, 1fr); }
        .live-row { grid-template-columns: minmax(0, 1fr) auto; }
        .calibrate-row, .reading, .trim-row { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }
        .channel-row { grid-template-columns: max-content minmax(0, 1fr); }
        .channel-row .field, .channel-row .hint { grid-column: 1 / -1; }
      }
    `),
  ];

  /** The effects of the parts that are rebuilt when the setup changes, disposed with them. */
  readonly #rebuilt: (() => void)[] = [];

  #renew(): (fn: () => void) => void {
    for (const dispose of this.#rebuilt.splice(0)) dispose();
    return (fn: () => void) => {
      this.#rebuilt.push(effect(fn));
    };
  }

  protected override render(): void {
    const store = useStore();
    const model = store.aggregate;
    this.onDisconnect(model.activate());
    this.onDisconnect(() => {
      for (const dispose of this.#rebuilt.splice(0)) dispose();
    });

    const verdict = h("span", { class: "verdict", "data-testid": "aggregate-verdict", "data-explain": "aggregate.verdict" }, "Reading...");
    const state = h("span", { class: "muted", "data-testid": "aggregate-state" });
    const again = h(
      "button",
      { type: "button", "data-testid": "aggregate-refresh", "data-explain": "aggregate.refresh", title: "Ask the server again now", "on:click": () => void model.refresh() },
      "Read again",
    );
    const readAt = h("span", { class: "muted", "data-testid": "aggregate-read-at" });
    const problem = h("p", { class: "note warning", role: "alert", "data-testid": "aggregate-problem", hidden: true });
    const outcome = h("p", { class: "note", role: "status", "data-testid": "aggregate-outcome", "data-explain": "aggregate.outcome", hidden: true });

    const reasons = h("ul", { class: "reasons", "data-testid": "aggregate-reasons" });
    const noReasons = h("p", { class: "note", "data-testid": "aggregate-no-reasons", hidden: true }, "Nothing is in the way. A DAW can open Gazelle Aggregate and every interface under it is ready.");
    const readySection = h(
      "ga-section",
      { heading: "Ready to use", explain: "aggregate.ready" },
      reasons,
      noReasons,
      outcome,
      h("p", { class: "note" }, "An aggregate needs each interface on its own USB host controller, one digital cable between them, and every interface clocked from that cable rather than from USB."),
    );

    const registration = h("dl", { class: "fields", "data-testid": "aggregate-registration" });
    const registerButton = h("button", { type: "button", "data-testid": "aggregate-register", "data-explain": "aggregate.register" }, "Register the driver");
    const unregisterButton = h("button", { type: "button", "data-testid": "aggregate-unregister", "data-explain": "aggregate.unregister" }, "Unregister");
    registerButton.addEventListener("click", () => void model.setRegistered(true));
    this.onDisconnect(bindConfirm(unregisterButton, "Unregister", () => void model.setRegistered(false)));
    const command = h("code", { "data-testid": "aggregate-command" });
    const registrationSection = h(
      "ga-section",
      { heading: "Registration", explain: "aggregate.registration" },
      registration,
      h("p", { class: "note" }, h("span", { "data-testid": "aggregate-admin-note" }, "Registering the driver writes to the part of Windows that lists audio drivers for every program on this PC, so Windows puts up its own administrator prompt. It is the one thing in Gazelle that asks for administrator rights, and declining the prompt changes nothing.")),
      h("div", { class: "add" }, registerButton, unregisterButton),
      h("p", { class: "note" }, "Or run this yourself, in a prompt started as an administrator: ", command),
    );

    const devices = h("div", { "data-testid": "aggregate-devices" });
    const noDevices = h("p", { class: "note", "data-testid": "aggregate-no-devices", hidden: true }, "No interfaces have been chosen yet. Add the ones the aggregate should open, in the order their channels should appear to a DAW.");
    const matchButton = h("button", { type: "button", "data-testid": "aggregate-match", "data-explain": "aggregate.match" }, "Match buffer sizes");
    this.onDisconnect(
      bindConfirm(matchButton, "Match buffer sizes", () => {
        const size = matchTarget(model.answer.peek());
        if (size !== undefined) void model.matchBuffers(size);
      }),
    );
    const addSelect = h("select", { "aria-label": "Interface to add", "data-testid": "aggregate-add-select", "data-no-wheel": true, "data-explain": "aggregate.add-select" });
    const addButton = h("button", { type: "button", "data-testid": "aggregate-add", "data-explain": "aggregate.add", "on:click": () => this.#addDevice(store, addSelect.value) }, "Add");
    const devicesSection = h(
      "ga-section",
      { heading: "Interfaces", explain: "aggregate.devices" },
      noDevices,
      devices,
      h("div", { class: "add" }, addSelect, addButton, h("span", { class: "spacer" }), matchButton),
      h("p", { class: "note" }, `Buffer size and Safe Mode are the audio driver's own settings on this PC, the same ones the Devices page shows. Changing either, or matching them, takes a confirming click, because ${RESTARTS}.`),
    );

    const setup = h("div", { class: "setup", "data-testid": "aggregate-setup" });
    const exportPath = h("code", { "data-testid": "aggregate-export-path" });
    const setupSection = h(
      "ga-section",
      { heading: "Setup", explain: "aggregate.setup" },
      setup,
      h("p", { class: "note" }, "Saved in the workspace, and written out for the driver at ", exportPath, ". The driver picks up a change at once when nothing is streaming, and at the next buffer change when a DAW is running."),
    );

    const calibrateSection = this.#buildCalibrate(store);

    const plan = h("dl", { class: "fields", "data-testid": "aggregate-plan" });
    const live = h("div", { class: "live", "data-testid": "aggregate-live" });
    const liveNote = h("p", { class: "note", "data-testid": "aggregate-live-note" });
    const liveSection = h("ga-section", { heading: "While a DAW has it open", explain: "aggregate.live" }, liveNote, plan, live);

    const events = h("ul", { class: "events", "data-testid": "aggregate-events" });
    const eventsNote = h("p", { class: "note", "data-testid": "aggregate-events-note" });
    const eventsSection = h("ga-section", { heading: "What happened", explain: "aggregate.events" }, eventsNote, events);

    const unavailable = h(
      "p",
      { class: "placeholder", "data-testid": "aggregate-unavailable", hidden: true },
      "This server does not offer the aggregate driver. It is answered only to a program on the same PC, so a Gazelle reached over the network shows nothing here.",
    );
    const sections = h("div", { "data-testid": "aggregate-sections" }, readySection, registrationSection, devicesSection, setupSection, calibrateSection, liveSection, eventsSection);

    this.root.replaceChildren(h("div", { class: "bar" }, verdict, state, h("span", { class: "spacer" }), readAt, again), problem, unavailable, sections);

    // Whether the server offers any of this at all. A server bound off loopback is not a failure.
    this.watch(() => {
      const offered = model.offered.value;
      unavailable.hidden = offered !== false;
      sections.hidden = offered === false;
      again.hidden = offered === false;
      verdict.hidden = offered === false;
      state.hidden = offered === false;
    });

    this.watch(() => {
      const why = model.problem.value;
      problem.hidden = why === undefined;
      problem.textContent = why ?? "";
    });

    this.watch(() => {
      const said = model.outcome.value;
      outcome.hidden = said === undefined;
      outcome.textContent = said?.text ?? "";
      outcome.classList.toggle("warning", said?.problem === true);
    });

    this.watch(() => {
      const busy = model.busy.value;
      for (const button of [registerButton, unregisterButton, matchButton, addButton]) button.disabled = busy !== undefined;
      for (const button of reasons.querySelectorAll<HTMLButtonElement>("button.fix")) button.disabled = busy !== undefined;
    });

    this.watch(() => {
      const answer = model.answer.value;
      if (answer === undefined) return;
      verdict.textContent = answer.ready ? "Ready" : "Not ready";
      verdict.setAttribute("data-ready", String(answer.ready));
      state.textContent = statusLine(answer.status);
      readAt.textContent = `Read at ${new Date(answer.read_at_ms).toLocaleTimeString()}.`;
      this.#showReasons(model, reasons, noReasons, answer);
      this.#showRegistration(registration, command, registerButton, unregisterButton, answer);
      this.#showPlan(plan, live, liveNote, answer);
      this.#showEvents(events, eventsNote, answer);
      matchButton.hidden = answer.devices.length < 2;
      matchButton.title = buffersMatch(answer) ? "Every interface is already on one buffer size" : `Put every interface on ${matchTarget(answer) ?? "one"} samples: ${RESTARTS}`;
    });

    // The cards and the setup are rebuilt only when the setup itself changes, so a poll landing
    // does not take away a name half typed or a menu half chosen.
    let shown: string | undefined;
    this.watch(() => {
      const config = store.workspace.value?.aggregate;
      noDevices.hidden = (config?.devices ?? []).length > 0;
      // Only the list itself decides what is built; everything else about the setup is followed by
      // the controls' own effects, so a rate chosen elsewhere does not throw away a name half typed.
      const shape = JSON.stringify(config?.devices ?? []);
      if (shape === shown) return;
      shown = shape;
      const watch = this.#renew();
      this.#buildDevices(store, devices, config, watch);
      this.#buildSetup(store, setup, config, watch);
    });

    this.watch(() => {
      const answer = model.answer.value;
      exportPath.textContent = answer?.export_path ?? "";
      this.#fillAdd(store, addSelect, addButton, answer);
    });
  }

  // -------------------------------------------------------------------------------------------
  // Is it usable
  // -------------------------------------------------------------------------------------------

  /**
   * The reasons, rebuilt only when they change. A row carries an armed Confirm and the answer is
   * read every second, so replacing the rows on every poll would take a first click away again
   * before the second one could land.
   */
  #reasonsShown: string | undefined;

  #showReasons(model: Store["aggregate"], list: HTMLElement, none: HTMLElement, answer: AggregateAnswer): void {
    none.hidden = answer.reasons.length > 0;
    list.hidden = answer.reasons.length === 0;
    const shown = JSON.stringify(answer.reasons);
    if (shown === this.#reasonsShown) return;
    this.#reasonsShown = shown;
    list.replaceChildren(...answer.reasons.map((reason) => this.#reasonRow(model, reason)));
  }

  #reasonRow(model: Store["aggregate"], reason: AggregateReason): HTMLElement {
    const severity = h(
      "span",
      { class: "severity", "data-severity": reason.severity, "data-testid": `reason-severity-${reason.code}`, "data-explain": "aggregate.severity" },
      reason.severity === "blocking" ? "STOPS IT" : "WORTH KNOWING",
    );
    const text = h("span", { "data-testid": `reason-${reason.code}` }, reason.message);
    const fix = reason.fix === undefined ? undefined : this.#fixButton(model, reason, reason.fix);
    return h("li", { class: "reason", "data-testid": `reason-row-${reason.code}` }, severity, text, ...(fix === undefined ? [] : [fix]));
  }

  /**
   * The button for a reason the server already knows how to put right. It sends the request the
   * answer named and nothing else; one that interrupts a DAW asks for a second click first.
   */
  #fixButton(model: Store["aggregate"], reason: AggregateReason, fix: AggregateFix): HTMLElement {
    const button = h("button", { type: "button", class: "fix", "data-testid": `reason-fix-${reason.code}`, "data-explain": "aggregate.fix", title: fix.label }, fix.label);
    if (fixNeedsConfirming(fix)) {
      button.title = `${fix.label}: ${RESTARTS}`;
      // The disarm is not kept: the row is thrown away whole when the reasons change, and a
      // timer of three seconds on a button that is gone has nothing left to arm.
      bindConfirm(button, fix.label, () => void model.applyFix(fix));
    } else {
      button.addEventListener("click", () => void model.applyFix(fix));
    }
    return button;
  }

  // -------------------------------------------------------------------------------------------
  // Registration
  // -------------------------------------------------------------------------------------------

  #showRegistration(fields: HTMLElement, command: HTMLElement, register: HTMLButtonElement, unregister: HTMLButtonElement, answer: AggregateAnswer): void {
    const { registration } = answer;
    const search = registration.dll_search;
    const rows: [label: string, value: string, key: string, testid: string][] = [
      ["Registered", registration.registered ? "Yes" : "No", "aggregate.registered", "registered"],
      ["What a DAW lists it as", registration.name, "aggregate.driver-name", "driver-name"],
      ["Registration points at", registration.dll ?? "Nothing: it is not registered", "aggregate.registered-dll", "registered-dll"],
      ["That file is there", registration.registered ? (registration.dll_present ? "Yes" : "No, so a DAW opening it would fail") : "Nothing is registered", "aggregate.dll-present", "dll-present"],
      ["The copy Gazelle would register", search.state === "found" ? search.dll : search.message, "aggregate.found-dll", "found-dll"],
    ];
    fields.replaceChildren(
      ...rows.flatMap(([label, value, key, testid]) => [
        h("dt", {}, label),
        h("dd", {}, h("span", { class: "readout", "data-testid": `aggregate-${testid}`, "data-explain": key }, value)),
      ]),
    );
    register.hidden = registration.registered && registration.dll_present;
    register.textContent = registration.registered ? "Register it again" : "Register the driver";
    unregister.hidden = !registration.registered;
    command.textContent = (registration.registered ? registration.unregister_command : registration.register_command) ?? "There is no copy of the driver on this PC to register.";
  }

  // -------------------------------------------------------------------------------------------
  // The interfaces: what each one is doing, and how to change it
  // -------------------------------------------------------------------------------------------

  #buildDevices(store: Store, into: HTMLElement, config: Aggregate | undefined, watch: (fn: () => void) => void): void {
    const list = config?.devices ?? [];
    into.replaceChildren(...list.map((device, index) => this.#deviceCard(store, device, index, list.length, watch)));
  }

  #deviceCard(store: Store, device: AggregateDevice, index: number, total: number, watch: (fn: () => void) => void): HTMLElement {
    const model = store.aggregate;
    const named = device.name ?? device.key ?? device.clsid ?? `Interface ${index + 1}`;
    const testid = `device-${index}`;

    const name = h("input", { class: "name", "aria-label": "Name for this interface", placeholder: device.key ?? "Interface", "data-testid": `${testid}-name`, "data-explain": "aggregate.device-name" });
    const showName = commitOnEnter(
      name,
      (value) => this.#editDevice(store, index, (current) => withField(current, "name", value.trim() === "" ? undefined : value.trim())),
      () => device.name ?? "",
      store.view<string | undefined>(`draft:aggregate:${index}:name`, undefined),
    );
    showName(device.name ?? "");

    const up = h("button", { type: "button", class: "order", "data-testid": `${testid}-up`, "data-explain": "aggregate.device-up", "aria-label": `Move ${named} earlier`, title: "Earlier: its channels come before the others", "on:click": () => this.#move(store, index, -1) }, "↑");
    const down = h("button", { type: "button", class: "order", "data-testid": `${testid}-down`, "data-explain": "aggregate.device-down", "aria-label": `Move ${named} later`, title: "Later: its channels come after the others", "on:click": () => this.#move(store, index, 1) }, "↓");
    up.disabled = index === 0;
    down.disabled = index === total - 1;
    const remove = h("button", { type: "button", "data-testid": `${testid}-remove`, "data-explain": "aggregate.device-remove", "aria-label": `Take ${named} out of the aggregate` }, "Remove");
    this.onDisconnect(bindConfirm(remove, "Remove", () => this.#removeDevice(store, index)));

    const which = h("span", { class: "readout", "data-testid": `${testid}-entry`, "data-explain": "aggregate.device-entry" }, device.key ?? device.clsid ?? "Not named");

    // Which of Gazelle's devices this is: what makes its clock, rate and buffer readable at all.
    // The answer settles it whether or not anybody has chosen, and this follows the answer.
    let resolvedId = typeof device.device_id === "string" ? device.device_id : undefined;
    /** The device whose driver has been asked for, so a resolved id asks for it exactly once. */
    let loadedDriver: string | undefined;
    const idSelect = h("select", { "aria-label": `Which connected device ${named} is`, "data-testid": `${testid}-device-id`, "data-no-wheel": true, "data-explain": "aggregate.device-id" });
    idSelect.addEventListener("change", () => this.#editDevice(store, index, (current) => withField(current, "device_id", idSelect.value === "" ? undefined : idSelect.value)));
    const worked = h("span", { class: "readout worked", "data-testid": `${testid}-worked-out`, "data-explain": "aggregate.device-worked-out", hidden: true }, "Worked out; choosing pins it");
    const notMatched = h("p", { class: "note warning", "data-testid": `${testid}-match-note`, hidden: true });

    const channels = h("span", { class: "readout", "data-testid": `${testid}-channels`, "data-explain": "aggregate.device-channels" });
    const clock = h("span", { class: "readout", "data-testid": `${testid}-clock`, "data-explain": "aggregate.device-clock" });
    const lock = h("span", { class: "lock", "data-testid": `${testid}-lock`, "data-explain": "aggregate.device-lock" }, "LOCK");
    const rate = h("span", { class: "readout", "data-testid": `${testid}-rate`, "data-explain": "aggregate.device-rate" });
    const gap = h("span", { class: "readout gap", "data-testid": `${testid}-gap`, "data-explain": "aggregate.device-gap" });

    // The driver's buffer size and Safe Mode, exactly as the Devices page offers them.
    const bufferMenu = h("select", { "aria-label": `Buffer size for ${named}`, "data-testid": `${testid}-buffer`, "data-no-wheel": true, "data-explain": "aggregate.device-buffer" });
    const bufferChoice = confirmedChoice(
      bufferMenu,
      `${testid}-buffer-confirm`,
      "aggregate.device-buffer-confirm",
      (size) => `Put ${named}'s driver on ${size} samples: ${RESTARTS}`,
      (size) => {
        if (resolvedId !== undefined) void store.setDriver(resolvedId, { buffer_size: size });
      },
    );
    this.onDisconnect(bufferChoice.disarm);

    let safeMode = false;
    const safe = h("button", { type: "button", class: "safe", "aria-label": `Safe Mode for ${named}`, "aria-pressed": "false", "data-testid": `${testid}-safe`, "data-explain": "aggregate.device-safe" }, "Off");
    this.onDisconnect(
      bindConfirm(safe, () => (safeMode ? "On" : "Off"), () => {
        if (resolvedId !== undefined) void store.setDriver(resolvedId, { safe_mode: !safeMode });
      }),
    );

    const inTrim = h("input", { class: "trim", type: "number", step: "1", "aria-label": `Input trim for ${named}, in samples`, "data-testid": `${testid}-in-trim`, "data-explain": "aggregate.device-in-trim" });
    const outTrim = h("input", { class: "trim", type: "number", step: "1", "aria-label": `Output trim for ${named}, in samples`, "data-testid": `${testid}-out-trim`, "data-explain": "aggregate.device-out-trim" });
    inTrim.value = String(device.input_trim ?? 0);
    outTrim.value = String(device.output_trim ?? 0);
    inTrim.addEventListener("change", () => this.#editDevice(store, index, (current) => withField(current, "input_trim", trimOf(inTrim.value))));
    outTrim.addEventListener("change", () => this.#editDevice(store, index, (current) => withField(current, "output_trim", trimOf(outTrim.value))));

    // Every channel of this interface: which of them the aggregate exposes, and what a DAW calls
    // each one. Closed, it is the one line that says what is exposed and how many are named.
    const inputRows = h("div", { "data-testid": `${testid}-inputs` });
    const outputRows = h("div", { "data-testid": `${testid}-outputs` });
    const unknownChannels = h(
      "p",
      { class: "note", "data-testid": `${testid}-channels-unknown`, hidden: true },
      "How many channels this interface has is not known until a DAW opens the aggregate or Gazelle knows which device it is.",
    );
    const channelsNote = h(
      "p",
      { class: "note", "data-testid": `${testid}-channels-note`, hidden: true },
      "A name given here is what a DAW shows, with the automatic name in brackets after it, and Gazelle's own names are only a suggestion, because the audio driver may put its channels in another order.",
    );
    const channelsPart = h(
      "details",
      { class: "channels", "data-testid": `${testid}-channels-part` },
      h("summary", { "data-testid": `${testid}-channels-open`, "data-explain": "aggregate.device-channels-open" }, h("span", { class: "label" }, "Channels"), channels),
      channelsNote,
      unknownChannels,
      inputRows,
      outputRows,
    );
    // Open or closed is this tab's, not the workspace's, so a poll or a rebuild leaves it as it was.
    const opened = store.view<boolean>(`aggregate:${index}:channels-open`, false);
    channelsPart.open = opened.peek();
    channelsPart.addEventListener("toggle", () => {
      opened.value = channelsPart.open;
    });

    const cell = (label: string, ...value: (Node | string)[]) => h("div", { class: "cell" }, h("span", { class: "label" }, label), h("span", {}, ...value));

    const card = h(
      "div",
      { class: "device-card", "data-testid": `aggregate-${testid}` },
      h("div", { class: "device-head" }, h("span", { class: "label" }, `${index + 1}.`), name, which, h("span", { class: "spacer" }), up, down, remove),
      h(
        "div",
        { class: "device-grid" },
        cell("Gazelle device", h("span", { class: "choice" }, idSelect, worked)),
        cell("Clock", clock, " ", lock),
        cell("Rate", rate),
        cell("Buffer", h("span", { class: "choice" }, bufferMenu, bufferChoice.confirm)),
        cell("Safe Mode", safe),
        cell("Input trim", inTrim),
        cell("Output trim", outTrim),
        cell("Gap", gap),
      ),
      notMatched,
      channelsPart,
    );

    watch(() => {
      // Which device this is, as the answer resolved it, and the menu of the connected ones with
      // whatever the setup names kept even when it is away.
      const view = viewFor(model.answer.value, device, index);
      resolvedId = resolvedDeviceId(view, device);
      const how = matchedBy(view);
      const attached = store.devices.value;
      const options = [h("option", { value: "" }, "Work it out")];
      for (const found of attached) options.push(h("option", { value: found.id }, `${found.model ?? found.id} (${found.id})`));
      if (resolvedId !== undefined && !attached.some((found) => found.id === resolvedId)) options.push(h("option", { value: resolvedId }, `${resolvedId} (not connected)`));
      idSelect.replaceChildren(...options);
      idSelect.value = resolvedId ?? "";
      idSelect.disabled = !store.connected.value;
      worked.hidden = how !== "worked_out";
      const why = matchNote(view);
      notMatched.hidden = why === undefined;
      notMatched.textContent = why ?? "";
      if (resolvedId !== undefined && resolvedId !== loadedDriver) {
        loadedDriver = resolvedId;
        void store.loadDriver(resolvedId);
      }
    });

    watch(() => {
      const view = viewFor(model.answer.value, device, index);
      const report = view?.report;
      clock.textContent = report?.clock?.source ?? (report?.attached === true ? "Not reported" : "Not connected");
      lock.toggleAttribute("data-locked", report?.clock?.locked === true);
      lock.textContent = report?.clock?.locked === true ? "LOCK" : "NO LOCK";
      rate.textContent = report?.clock?.hz === undefined ? "Not known" : `${Number((report.clock.hz / 1000).toFixed(3))} kHz`;
      const reading = gapFor(view);
      gap.textContent = reading.text;
      gap.setAttribute("data-tone", reading.tone);
    });

    watch(() => {
      // The sizes the driver offers, read as the Devices page reads them; its own reading wins.
      // The device is the one the answer resolved, so this is live without anybody choosing one.
      const view = viewFor(model.answer.value, device, index);
      const id = resolvedDeviceId(view, device);
      const controls = id === undefined ? undefined : driverControls(store.driver(id).value);
      const summary = view?.report?.driver;
      const sizes = controls?.sizes ?? BUFFER_SIZES;
      const current = controls?.buffer ?? summary?.buffer_size;
      if ([...bufferMenu.options].map((option) => Number(option.value)).join() !== sizes.join()) {
        bufferMenu.replaceChildren(...sizes.map((size) => h("option", { value: String(size) }, `${size} samples`)));
      }
      if (current !== undefined) {
        bufferChoice.current = current;
        if (!bufferChoice.armed()) bufferMenu.value = String(current);
      }
      bufferMenu.disabled = id === undefined || current === undefined || !store.connected.value;
      safeMode = controls?.safeMode ?? summary?.safe_mode ?? false;
      safe.setAttribute("aria-pressed", String(safeMode));
      if (!safe.hasAttribute("data-armed")) safe.textContent = summary?.safe_mode === undefined && controls === undefined ? "Not read" : safeMode ? "On" : "Off";
      safe.disabled = id === undefined || !store.connected.value;
      for (const field of [inTrim, outTrim, name]) field.disabled = !store.connected.value;
    });

    // The channel rows are rebuilt only when how many there are, or what Gazelle calls them,
    // changes: a poll landing every second must not take away a label half typed.
    let built: string | undefined;
    watch(() => {
      const view = viewFor(model.answer.value, device, index);
      const counts = channelCounts(view);
      const names = view?.report?.channels;
      channels.textContent = channelSummary(device, counts);
      channels.title = device.inputs === undefined && device.outputs === undefined ? "Every channel the interface has" : "Only the channels the workspace names";
      const known = counts.inputs !== undefined || counts.outputs !== undefined;
      unknownChannels.hidden = known;
      channelsNote.hidden = !known;
      const shape = JSON.stringify([counts, names]);
      if (shape !== built) {
        built = shape;
        inputRows.replaceChildren(...this.#channelRows(store, device, index, named, true, counts.inputs, names));
        outputRows.replaceChildren(...this.#channelRows(store, device, index, named, false, counts.outputs, names));
      }
      const connected = store.connected.value;
      for (const control of channelsPart.querySelectorAll<HTMLButtonElement | HTMLInputElement>("button.expose, input.field")) control.disabled = !connected;
    });

    return card;
  }

  /**
   * One side of an interface's channels: a heading and a row each, or nothing at all when how many
   * there are is not known. A count that is not known offers nothing rather than guessing at one.
   */
  #channelRows(store: Store, device: AggregateDevice, index: number, named: string, input: boolean, count: number | undefined, names: AggregateChannelNames | undefined): Node[] {
    if (count === undefined) return [];
    const heading = h("p", { class: "side" }, input ? "INPUTS" : "OUTPUTS");
    return [heading, ...Array.from({ length: count }, (_, channel) => this.#channelRow(store, device, index, named, input, channel, count, names))];
  }

  /**
   * One channel: what it is called by itself, whether the aggregate exposes it, and the name the
   * person gives it, which a DAW shows with the automatic one in brackets after it.
   */
  #channelRow(store: Store, device: AggregateDevice, index: number, named: string, input: boolean, channel: number, count: number, names: AggregateChannelNames | undefined): HTMLElement {
    const side = input ? "in" : "out";
    const testid = `device-${index}-${side}-${channel}`;
    const listKey = input ? "inputs" : "outputs";
    const nameKey = input ? "input_names" : "output_names";
    const auto = autoChannelName(named, channel);
    const exposed = isExposed(device[listKey], channel);

    const expose = h(
      "button",
      {
        type: "button",
        class: "expose",
        "aria-pressed": String(exposed),
        "aria-label": `Expose ${auto}`,
        title: exposed ? `${auto} is one of the channels a DAW sees` : `${auto} is kept out of what a DAW sees`,
        "data-testid": `${testid}-expose`,
        "data-explain": "aggregate.channel-expose",
        "on:click": () => this.#editDevice(store, index, (current) => withField(current, listKey, withChannelExposed(current[listKey] as number[] | undefined, count, channel, !exposed))),
      },
      exposed ? "On" : "Off",
    );

    const label = h("input", {
      class: "field",
      maxlength: String(CHANNEL_LABEL_MAX),
      placeholder: auto,
      "aria-label": `Name for ${auto}`,
      "data-testid": `${testid}-label`,
      "data-explain": "aggregate.channel-label",
    });
    const showLabel = commitOnEnter(
      label,
      (value) => this.#editDevice(store, index, (current) => withField(current, nameKey, withChannelName(current[nameKey] as Record<string, string> | undefined, channel, value))),
      () => channelLabel(device[nameKey], channel),
      store.view<string | undefined>(`draft:aggregate:${index}:${side}:${channel}`, undefined),
    );
    showLabel(channelLabel(device[nameKey], channel));

    const hint = suggestedChannelName(names, input, channel);
    return h(
      "div",
      { class: "channel-row", "data-testid": testid },
      expose,
      h("span", { class: "auto readout", "data-testid": `${testid}-auto`, "data-explain": "aggregate.channel-auto" }, auto),
      label,
      h("span", { class: "hint readout", "data-testid": `${testid}-hint`, "data-explain": "aggregate.channel-hint" }, hint === undefined ? "" : `Gazelle calls it ${hint}`),
    );
  }

  // -------------------------------------------------------------------------------------------
  // Editing the setup
  // -------------------------------------------------------------------------------------------

  #buildSetup(store: Store, into: HTMLElement, config: Aggregate | undefined, watch: (fn: () => void) => void): void {
    const devices = config?.devices ?? [];
    const master = h("select", { "aria-label": "Which interface drives the callback", "data-testid": "aggregate-master", "data-no-wheel": true, "data-explain": "aggregate.master" });
    master.replaceChildren(
      h("option", { value: "" }, "The first one"),
      ...devices.map((device, index) => h("option", { value: nameOf(device, index) }, nameOf(device, index))),
    );
    master.addEventListener("change", () => store.editAggregate((current) => withField(current, "callback_master", master.value === "" ? undefined : master.value)));

    const alignment = h(
      "select",
      { "aria-label": "How the streams line up", "data-testid": "aggregate-alignment", "data-no-wheel": true, "data-explain": "aggregate.alignment" },
      h("option", { value: "aligned" }, "Aligned: every interface in step"),
      h("option", { value: "lowest_latency" }, "Lowest latency: no padding"),
    );
    alignment.addEventListener("change", () => store.editAggregate((current) => ({ ...current, alignment: alignment.value === "lowest_latency" ? "lowest_latency" : "aligned" })));

    const rate = h(
      "select",
      { "aria-label": "Sample rate for the aggregate", "data-testid": "aggregate-rate", "data-no-wheel": true, "data-explain": "aggregate.rate" },
      h("option", { value: "" }, "Whatever the interfaces are on"),
      ...RATE_HZ.map((hz, index) => h("option", { value: String(hz) }, SAMPLE_RATES[index] as string)),
    );
    rate.addEventListener("change", () => store.editAggregate((current) => withField(current, "rate", rate.value === "" ? undefined : Number(rate.value))));

    const buffer = h(
      "select",
      { "aria-label": "Buffer size to offer a DAW", "data-testid": "aggregate-buffer", "data-no-wheel": true, "data-explain": "aggregate.buffer" },
      h("option", { value: "" }, "Whatever the drivers are on"),
      ...BUFFER_SIZES.map((size) => h("option", { value: String(size) }, `${size} samples`)),
    );
    buffer.addEventListener("change", () => store.editAggregate((current) => withField(current, "buffer_size", buffer.value === "" ? undefined : Number(buffer.value))));

    into.replaceChildren(
      h("span", { class: "label" }, "Callback master"),
      master,
      h("span", { class: "label" }, "Alignment"),
      alignment,
      h("span", { class: "label" }, "Sample rate"),
      rate,
      h("span", { class: "label" }, "Buffer size"),
      buffer,
    );

    watch(() => {
      const current = store.workspace.value?.aggregate;
      master.value = typeof current?.callback_master === "string" && devices.some((device, index) => nameOf(device, index) === current.callback_master) ? current.callback_master : "";
      alignment.value = current?.alignment === "lowest_latency" ? "lowest_latency" : "aligned";
      rate.value = typeof current?.rate === "number" && RATE_HZ.includes(current.rate) ? String(current.rate) : "";
      buffer.value = typeof current?.buffer_size === "number" && BUFFER_SIZES.includes(current.buffer_size) ? String(current.buffer_size) : "";
      for (const control of [master, alignment, rate, buffer]) control.disabled = !store.connected.value;
    });
  }

  #fillAdd(store: Store, select: HTMLSelectElement, add: HTMLButtonElement, answer: AggregateAnswer | undefined): void {
    const already = new Set((store.workspace.peek()?.aggregate?.devices ?? []).map((device) => (device.key ?? "").toLowerCase()));
    const offered = (answer?.drivers ?? []).filter((entry) => !entry.is_aggregate && !already.has(entry.key.toLowerCase()));
    select.replaceChildren(...(offered.length === 0 ? [h("option", { value: "" }, "No other audio driver on this PC")] : offered.map((entry) => h("option", { value: entry.key }, entry.description ?? entry.key))));
    select.disabled = offered.length === 0;
    add.disabled = offered.length === 0;
  }

  #addDevice(store: Store, key: string): void {
    if (key === "") return;
    store.editAggregate((current) => ({ ...current, devices: [...(current.devices ?? []), { key, name: key }] }));
  }

  #removeDevice(store: Store, index: number): void {
    store.editAggregate((current) => ({ ...current, devices: (current.devices ?? []).filter((_, at) => at !== index) }));
  }

  #move(store: Store, index: number, by: number): void {
    store.editAggregate((current) => {
      const devices = [...(current.devices ?? [])];
      const to = index + by;
      const moved = devices[index];
      const other = devices[to];
      if (moved === undefined || other === undefined) return current;
      devices[index] = other;
      devices[to] = moved;
      return { ...current, devices };
    });
  }

  #editDevice(store: Store, index: number, change: (device: AggregateDevice) => AggregateDevice): void {
    store.editAggregate((current) => {
      const devices = [...(current.devices ?? [])];
      const at = devices[index];
      if (at === undefined) return current;
      devices[index] = change(at);
      return { ...current, devices };
    });
  }

  // -------------------------------------------------------------------------------------------
  // Lining the interfaces up: measuring the trims rather than guessing at them
  // -------------------------------------------------------------------------------------------

  /**
   * The section that measures the trims. It plays one click through the aggregate itself and hears
   * where each interface puts it, so what is measured is exactly what the aggregate produces.
   *
   * Nothing here works out what it means: the store turns the pickers into the cabling to patch,
   * the request to send, the reason a run cannot be made, and what the readings and trims read as.
   * What is left here is the controls, and keeping them still while the page polls: the rows are
   * rebuilt only when the interfaces or the channels they offer change, and what has been chosen
   * lives in this tab's view state, so an answer landing never moves a menu.
   */
  #buildCalibrate(store: Store): HTMLElement {
    const model = store.aggregate;
    const chosen = store.view<CalibratePicks | undefined>("aggregate:calibrate", undefined);
    const picksNow = (): CalibratePicks => reconcilePicks(chosen.peek(), store.workspace.peek()?.aggregate, model.answer.peek());
    const change = (next: CalibratePicks) => {
      chosen.value = next;
    };
    const menu = (label: string, testid: string, explain: string) =>
      h("select", { "aria-label": label, "data-testid": testid, "data-no-wheel": true, "data-explain": explain });

    const direction = menu("Which way round to measure", "calibrate-direction", "aggregate.calibrate-direction");
    direction.replaceChildren(
      h("option", { value: "inputs" }, "Inputs: line up what the interfaces record"),
      h("option", { value: "outputs" }, "Outputs: line up what the interfaces play"),
    );
    direction.addEventListener("change", () => change({ ...picksNow(), direction: direction.value === "outputs" ? "outputs" : "inputs" }));

    const reference = menu("Which interface every cable has an end on", "calibrate-reference", "aggregate.calibrate-reference");
    reference.addEventListener("change", () => change({ ...picksNow(), reference: reference.value }));

    const clicks = menu("How many clicks to play", "calibrate-clicks", "aggregate.calibrate-clicks");
    clicks.replaceChildren(...CLICKS.map((count) => h("option", { value: String(count) }, `${count} clicks`)));
    clicks.addEventListener("change", () => change({ ...picksNow(), clicks: Number(clicks.value) }));

    const level = menu("How loud the click is", "calibrate-level", "aggregate.calibrate-level");
    level.replaceChildren(...LEVELS_DBFS.map((dbfs) => h("option", { value: String(dbfs) }, `${dbfs} dBFS`)));
    level.addEventListener("change", () => change({ ...picksNow(), level_dbfs: Number(level.value) }));

    const rows = h("div", { "data-testid": "calibrate-rows" });
    const cables = h("ul", { class: "cables", "data-testid": "calibrate-cables" });
    const problem = h("p", { class: "note warning", "data-testid": "calibrate-problem", hidden: true });

    const measure = h("button", { type: "button", "data-testid": "calibrate-measure", "data-explain": "aggregate.calibrate-measure" }, "Measure");
    this.onDisconnect(
      bindConfirm(measure, "Measure", () => {
        const request = calibrateRequest(picksNow(), store.workspace.peek()?.aggregate, model.answer.peek());
        if (request !== undefined) void model.startCalibration(request);
      }),
    );
    const stop = h("button", { type: "button", "data-testid": "calibrate-stop", "data-explain": "aggregate.calibrate-stop", hidden: true, "on:click": () => void model.stopCalibration() }, "Stop");
    const step = h("span", { class: "readout", "data-testid": "calibrate-step", "data-explain": "aggregate.calibrate-step" });
    const fill = h("span", { class: "fill" });
    const track = h("span", { class: "track" }, fill);
    const running = h("div", { class: "running", "data-testid": "calibrate-running", hidden: true }, step, track);

    const summary = h("p", { class: "note", "data-testid": "calibrate-summary", hidden: true });
    const drift = h("p", { class: "drift", role: "alert", "data-testid": "calibrate-drift", hidden: true });
    const readings = h("div", { "data-testid": "calibrate-readings" });
    const trims = h("div", { "data-testid": "calibrate-trims" });
    const apply = h("button", { type: "button", "data-testid": "calibrate-apply", "data-explain": "aggregate.calibrate-apply", hidden: true }, "Write these trims into the setup");
    apply.addEventListener("click", () => this.#applyTrims(store));
    const applied = h("p", { class: "note", role: "status", "data-testid": "calibrate-applied", hidden: true });
    const warnings = h("p", { class: "note warning", "data-testid": "calibrate-warnings", hidden: true });
    const refusal = h("p", { class: "note warning", role: "alert", "data-testid": "calibrate-refusal", hidden: true });

    const unavailable = h("p", { class: "note", "data-testid": "calibrate-unavailable", hidden: true }, "This server does not measure. It is a newer part of Gazelle than the server this page is talking to.");

    const section = h(
      "ga-section",
      { heading: "Line the interfaces up", explain: "aggregate.calibrate" },
      h(
        "p",
        { class: "note" },
        "The aggregate lines its interfaces up from the latency figures their own drivers report, and those figures are a little out, so two interfaces can still record a few tens of samples apart. This plays one click through the aggregate itself and hears where each interface puts it, which is the difference to put in the trims.",
      ),
      h("div", { class: "setup", "data-testid": "calibrate-setup" }, h("span", { class: "label" }, "Pass"), direction, h("span", { class: "label" }, "Reference"), reference, h("span", { class: "label" }, "Clicks"), clicks, h("span", { class: "label" }, "Level"), level),
      rows,
      h("p", { class: "note", "data-testid": "calibrate-patch-note" }, "Patch it like this, one cable a line, then press Measure twice:"),
      cables,
      h(
        "p",
        { class: "note warning", "data-testid": "calibrate-warning" },
        "It plays a click out of a real output at the level above, so turn monitors down first, and it takes both audio drivers for itself while it runs, so close any DAW that has them open. Measure asks for a confirming click.",
      ),
      problem,
      h("div", { class: "add" }, measure, stop),
      running,
      refusal,
      summary,
      drift,
      readings,
      trims,
      h("div", { class: "add" }, apply),
      applied,
      warnings,
      unavailable,
    );

    /** The row controls, rebuilt with the rows, so the watch can put the chosen channels back. */
    let controls: { row: HTMLElement; output: HTMLSelectElement; input: HTMLSelectElement }[] = [];
    let built: string | undefined;
    let shownOutcome: string | undefined;

    this.watch(() => {
      const config = store.workspace.value?.aggregate;
      const answer = model.answer.value;
      const picks = reconcilePicks(chosen.value, config, answer);
      const devices = calibrateDevices(config);
      const outputs = aggregateChannels(config, answer, false);
      const inputs = aggregateChannels(config, answer, true);

      // The interfaces menu, and one row per interface. Rebuilt only when what they offer changes.
      const shape = JSON.stringify([devices, picks.direction, picks.reference, outputs.map((one) => one.text), inputs.map((one) => one.text)]);
      if (shape !== built) {
        built = shape;
        reference.replaceChildren(...devices.map((device) => h("option", { value: device }, device)));
        controls = devices.map((device, at) => this.#calibrateRow(picks, device, at, devices, outputs, inputs));
        rows.replaceChildren(...controls.map(({ row }) => row));
        for (const [at, control] of controls.entries()) {
          control.output.addEventListener("change", () => change({ ...picksNow(), outputs: replaceAt(picksNow().outputs, at, Number(control.output.value)) }));
          control.input.addEventListener("change", () => change({ ...picksNow(), inputs: replaceAt(picksNow().inputs, at, Number(control.input.value)) }));
        }
      }
      direction.value = picks.direction;
      reference.value = picks.reference;
      clicks.value = String(picks.clicks);
      level.value = String(picks.level_dbfs);
      for (const [at, control] of controls.entries()) {
        control.output.value = picks.outputs[at] === undefined ? "" : String(picks.outputs[at]);
        control.input.value = picks.inputs[at] === undefined ? "" : String(picks.inputs[at]);
      }

      // What to plug in, named channel by channel.
      cables.replaceChildren(
        ...cablingSteps(picks, config, answer).map((cable, at) =>
          h("li", { "data-testid": `calibrate-cable-${at}` }, h("span", { class: "at" }, `${at + 1}.`), h("span", { class: "cable readout", "data-explain": "aggregate.calibrate-cable" }, cable.text)),
        ),
      );

      const why = calibrateProblem(picks, config, answer);
      problem.hidden = why === undefined;
      problem.textContent = why ?? "";

      const state = model.calibration.value;
      const offered = model.calibrationOffered.value;
      const isRunning = calibrateRunning(state);
      unavailable.hidden = offered !== false;
      running.hidden = !isRunning;
      step.textContent = calibrateStepText(state);
      fill.style.width = `${Math.round(calibrateProgress(state) * 100)}%`;
      stop.hidden = !isRunning;
      measure.hidden = isRunning;
      measure.disabled = why !== undefined || offered === false || !store.connected.value || model.busy.value !== undefined;
      for (const control of [direction, reference, clicks, level, ...controls.flatMap(({ output, input }) => [output, input])]) control.disabled = isRunning || !store.connected.value;

      const said = model.calibrationProblem.value ?? (state?.state === "failed" ? state.refusal : undefined);
      refusal.hidden = said === undefined;
      refusal.textContent = said ?? "";

      // What it measured, rebuilt only when the outcome itself changes.
      const outcome = state?.state === "done" ? state.outcome : undefined;
      const shown = JSON.stringify(outcome ?? null);
      if (shown !== shownOutcome) {
        shownOutcome = shown;
        summary.hidden = outcome === undefined;
        summary.textContent = outcome === undefined ? "" : outcomeSummary(outcome);
        drift.hidden = !driftFound(outcome);
        drift.textContent = driftFound(outcome) ? "The interfaces are not sharing one clock. A trim cannot answer that: it would be right now and wrong in a minute. Put every interface on the clock that comes down the digital cable, then measure again." : "";
        readings.replaceChildren(...(outcome?.readings ?? []).map((reading) => this.#readingRow(reading)));
        trims.replaceChildren(...trimRows(outcome).map((trim, at) => this.#trimRow(trim, at)));
        apply.hidden = trimsToApply(outcome).length === 0;
        warnings.hidden = (outcome?.warnings ?? []).length === 0;
        warnings.textContent = (outcome?.warnings ?? []).join(" ");
      }
      apply.disabled = !store.connected.value;
      const wrote = store.view<string | undefined>("aggregate:calibrate:applied", undefined).value;
      applied.hidden = wrote === undefined;
      applied.textContent = wrote ?? "";
    });

    return section;
  }

  /** One interface's row: the output that carries its click, and the input that records it. */
  #calibrateRow(
    picks: CalibratePicks,
    device: string,
    at: number,
    devices: string[],
    outputs: ReturnType<typeof aggregateChannels>,
    inputs: ReturnType<typeof aggregateChannels>,
  ): { row: HTMLElement; output: HTMLSelectElement; input: HTMLSelectElement } {
    const pick = (side: "outputs" | "inputs", list: ReturnType<typeof aggregateChannels>, label: string, explain: string) => {
      const on = slotDevice(picks.direction, picks.reference, side, at, devices);
      const offered = channelsOf(list, on);
      const select = h("select", {
        "aria-label": `${label} for ${device}`,
        "data-testid": `calibrate-${side === "outputs" ? "plays" : "records"}-${at}`,
        "data-no-wheel": true,
        "data-explain": explain,
      });
      select.replaceChildren(
        ...(offered.length === 0
          ? [h("option", { value: "" }, `No channel of ${on} is known yet`)]
          : offered.map((channel) => h("option", { value: String(channel.number) }, channel.text))),
      );
      return select;
    };
    const output = pick("outputs", outputs, "Plays the click", "aggregate.calibrate-plays");
    const input = pick("inputs", inputs, "Records the click", "aggregate.calibrate-records");
    const row = h(
      "div",
      { class: "calibrate-row", "data-testid": `calibrate-row-${at}` },
      h("span", { class: "who" }, device),
      h("span", { class: "cell" }, h("span", { class: "label" }, "Plays"), output),
      h("span", { class: "cell" }, h("span", { class: "label" }, "Records"), input),
    );
    return { row, output, input };
  }

  /** One interface's reading: how far out, how steady, and how much it had to go on. */
  #readingRow(reading: Parameters<typeof readingView>[0]): HTMLElement {
    const view = readingView(reading);
    const row = h(
      "div",
      { class: "reading", "data-testid": `calibrate-reading-${view.device}` },
      h("span", {}, view.device, reading.is_reference ? " (reference)" : ""),
      h("span", { class: "readout lag", "data-tone": view.tone, "data-testid": `calibrate-lag-${view.device}`, "data-explain": "aggregate.calibrate-lag" }, view.lag),
      h("span", { class: "readout", "data-testid": `calibrate-spread-${view.device}`, "data-explain": "aggregate.calibrate-spread" }, view.spread),
      h("span", { class: "readout", "data-testid": `calibrate-clicks-${view.device}`, "data-explain": "aggregate.calibrate-found" }, view.clicks),
    );
    const after: Node[] = [];
    // The server's own sentence for this interface, and the drift finding, which is the serious one.
    if (view.note !== undefined) after.push(h("p", { class: "note", "data-testid": `calibrate-note-${view.device}` }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-note" }, view.note)));
    if (view.drift !== undefined) after.push(h("p", { class: "drift", "data-testid": `calibrate-drift-${view.device}`, "data-explain": "aggregate.calibrate-drift" }, view.drift));
    return after.length === 0 ? row : h("div", {}, row, ...after);
  }

  /** One trim the measurement implies: what it is now, what was measured, and what it would become. */
  #trimRow(trim: ReturnType<typeof trimRows>[number], at: number): HTMLElement {
    const cell = (label: string, value: string, testid: string, explain: string) =>
      h("span", { class: "cell" }, h("span", { class: "label" }, label), h("span", { class: "readout", "data-testid": testid, "data-explain": explain }, value));
    const row = h(
      "div",
      { class: "trim-row", "data-testid": `calibrate-trim-${at}` },
      h("span", {}, `${trim.device}, ${trim.what.toLowerCase()}`),
      cell("Now", trim.was, `calibrate-trim-${at}-was`, "aggregate.calibrate-trim-was"),
      cell("Measured", trim.measured, `calibrate-trim-${at}-measured`, "aggregate.calibrate-trim-measured"),
      cell("Would be", trim.now, `calibrate-trim-${at}-now`, "aggregate.calibrate-trim-now"),
    );
    if (trim.notApplied === undefined) return row;
    // One the server is not offering: it is shown, with its reason, and the button passes it over.
    return h("div", {}, row, h("p", { class: "note", "data-testid": `calibrate-trim-${at}-not-applied` }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-not-applied" }, trim.notApplied)));
  }

  /** Writes the measured trims into the setup, the same way every other field on this page does. */
  #applyTrims(store: Store): void {
    const outcome = store.aggregate.calibration.peek()?.outcome;
    const changing = trimsToApply(outcome);
    store.editAggregate((current) => withMeasuredTrims(current, outcome));
    store.view<string | undefined>("aggregate:calibrate:applied", undefined).value = appliedTrimsText(changing);
  }

  // -------------------------------------------------------------------------------------------
  // Live, while a DAW has it open
  // -------------------------------------------------------------------------------------------

  #showPlan(fields: HTMLElement, rows: HTMLElement, note: HTMLElement, answer: AggregateAnswer): void {
    const status = answer.status;
    if (status.state !== "read") {
      note.textContent = status.message;
      fields.hidden = true;
      rows.replaceChildren();
      return;
    }
    note.textContent = statusLine(status);
    const plan = status.plan;
    fields.hidden = plan === undefined;
    if (plan !== undefined) {
      const row = (label: string, value: string, key: string, testid: string) => [h("dt", {}, label), h("dd", {}, h("span", { class: "readout", "data-testid": `plan-${testid}`, "data-explain": key }, value))];
      fields.replaceChildren(
        ...row("Master", plan.master, "aggregate.plan-master", "master"),
        ...row("Rate", `${Number((plan.rate / 1000).toFixed(3))} kHz`, "aggregate.plan-rate", "rate"),
        ...row("Buffer", `${plan.buffer_size} samples`, "aggregate.plan-buffer", "buffer"),
        ...row("Channels", `${plan.inputs} in, ${plan.outputs} out`, "aggregate.plan-channels", "channels"),
        ...row("Alignment", plan.alignment === "lowest_latency" ? "Lowest latency" : "Aligned", "aggregate.plan-alignment", "alignment"),
        ...row("Latency", `${plan.input_latency} in, ${plan.output_latency} out, in samples`, "aggregate.plan-latency", "latency"),
        ...(status.up_to_date ? [] : row("Setup in force", `Generation ${status.generation_in_force}, and Gazelle has asked for ${status.generation}`, "aggregate.plan-generation", "generation")),
        ...(status.last_refusal === undefined ? [] : row("Last refused", status.last_refusal, "aggregate.plan-refusal", "refusal")),
      );
    }
    rows.replaceChildren(
      ...status.devices.map((device) => {
        const reading = gapView(device);
        return h(
          "div",
          { class: "live-row", "data-testid": `live-${device.name}` },
          h("span", {}, device.name, device.is_master ? " (master)" : ""),
          h("span", { class: "readout gap", "data-tone": reading.tone, "data-testid": `live-gap-${device.name}`, "data-explain": "aggregate.live-gap" }, reading.text),
          h("span", { class: "readout", "data-testid": `live-callbacks-${device.name}`, "data-explain": "aggregate.live-callbacks" }, `${device.callbacks} blocks`),
          h("span", { class: "readout", "data-testid": `live-dropped-${device.name}`, "data-explain": "aggregate.live-dropped" }, `${device.dropped} dropped`),
          h("span", { class: "readout", "data-testid": `live-starved-${device.name}`, "data-explain": "aggregate.live-starved" }, `${device.starved} starved`),
        );
      }),
    );
  }

  #showEvents(list: HTMLElement, note: HTMLElement, answer: AggregateAnswer): void {
    if (answer.events_error !== undefined) {
      note.textContent = answer.events_error;
      list.replaceChildren();
      return;
    }
    note.textContent =
      answer.events.length === 0
        ? "The driver has written nothing yet. It writes a line only when something happens, so an empty log is a driver that has never run here."
        : "The driver's own log, newest last. It is kept on disk, so a session that would not start last night still says why today.";
    list.replaceChildren(
      ...answer.events.map((event) =>
        h(
          "li",
          { "data-testid": "aggregate-event" },
          h("span", { class: "at" }, event.at),
          h("span", { class: "kind" }, event.kind),
          h("span", {}, event.message),
        ),
      ),
    );
  }
}

/**
 * `{ ...base, [field]: value }`, except that an undefined value takes the field out rather than
 * putting it in as undefined. Both the workspace and the driver's file mean "not set" by the field
 * being absent, and a key written as null would be a value the driver would have to make sense of.
 */
function withField<T extends object, K extends keyof T>(base: T, field: K, value: T[K] | undefined): T {
  const next = { ...base };
  if (value === undefined) delete next[field];
  else next[field] = value;
  return next;
}

const nameOf = deviceName;

/** One entry of a list changed, for the per-interface channel a picker names. */
function replaceAt<T>(list: T[], at: number, value: T): T[] {
  const next = [...list];
  next[at] = value;
  return next;
}

/** A trim as the file takes it: a whole number of samples, and nothing at all for zero. */
function trimOf(value: string): number | undefined {
  const samples = Math.trunc(Number(value));
  return Number.isFinite(samples) && samples !== 0 ? samples : undefined;
}

/** The gap line for a card: the driver's figure, or why there is not one. */
function gapFor(view: AggregateDeviceView | undefined): { text: string; tone: string } {
  const live = view?.live;
  if (live === undefined) return { text: "No DAW has it open", tone: "idle" };
  return gapView(live);
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-aggregate": GaAggregate;
  }
}
