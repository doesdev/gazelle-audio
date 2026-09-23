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
// channels: which of them the aggregate exposes, and a name of the person's own for any of them.
//
// Everything is named one way, Gazelle's (the store's `aggregateNaming`): an interface by Gazelle's
// name for its device, and a channel, which is one of the interface's USB channels, first for what
// it carries and then by that USB channel. The vendor driver's own name appears once, as a detail.
//
// Which of Gazelle's devices an entry is comes from the answer, not from this page: the server
// resolves it the same way for every route, so an interface nobody has pinned still reads its
// clock, rate and buffer. Choosing one in the menu pins it, and Work it out gives it back.

import { h } from "../core/dom.ts";
import { effect, untracked } from "../core/signal.ts";
import {
  aggregateNaming,
  appliedTrimsText,
  buffersMatch,
  cablePort,
  cablingSteps,
  cardPhaseView,
  checkVerdicts,
  choicesWith,
  calibrateDevices,
  calibrateProblem,
  calibrateProgress,
  calibrateRequest,
  calibrateRunning,
  calibrateStepText,
  channelCounts,
  channelLabel,
  channelName,
  channelsOf,
  channelSummary,
  CHANNEL_LABEL_MAX,
  CLICKS,
  countCheck,
  dawLine,
  deviceViews,
  driftFound,
  driverOfferText,
  eventView,
  fixNeedsConfirming,
  gapView,
  interfaceChannels,
  interfaceName,
  interfaceNames,
  isExposed,
  LEVELS_DBFS,
  livePhaseView,
  masterIndex,
  masterReference,
  matchedBy,
  pageNameOf,
  playbackOutputs,
  rateInForceText,
  readGroups,
  recordingInputs,
  matchNote,
  matchTarget,
  outcomeSummary,
  phaseChoices,
  phaseFromPicks,
  phasePicks,
  phaseRefusedText,
  phaseRoutingNote,
  phaseSetting,
  phaseSetupView,
  readingView,
  reasonCard,
  reasonHint,
  reconcilePicks,
  resolvedDeviceId,
  runCleanText,
  runPhaseViews,
  slotDevice,
  statusLine,
  trimRows,
  trimsToApply,
  usbGroups,
  viewFor,
  withChannelExposed,
  withChannelName,
  withMeasuredTrims,
  withPageNames,
  withPass,
  withPhase,
  witnessViews,
  type Aggregate,
  type AggregateAnswer,
  type AggregateCalibrateDirection,
  type AggregateDevice,
  type AggregateDeviceView,
  type AggregateFix,
  type AggregateNaming,
  type AggregateReason,
  type CalibratePicks,
  type InterfaceNaming,
  type PhaseChoice,
  type PhasePicks,
  type PlaybackSend,
  type RunPhaseView,
  type VerdictView,
  type WitnessView,
} from "../store/aggregate.ts";
import { driverControls } from "../store/driver.ts";
import { displayName, SAMPLE_RATES, type Store } from "../store/store.ts";
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
      /* The head names the interface, by its name in Gazelle, and orders it. A long name wraps rather
         than pushing the buttons off the card. */
      .device-head { gap: 6px; }
      .device-head .name { flex: 0 1 auto; min-width: 0; font-weight: 700; overflow-wrap: anywhere; }
      .device-head .spacer { flex: 1; }
      .device-head .order { min-width: 26px; padding: 0 4px; }
      .live-row > * { min-width: 0; }
      /* Worked out for you rather than chosen: a hint beside the menu, not a value of its own. */
      .worked { min-width: 0; padding: 0; border: 0; background: none; font-size: 11px; color: var(--ga-text-muted); }
      /* The Channels part is the last row of the card's grid, so its label starts where every other
         label on the card does. Closed it is one line; open it is a row per channel. */
      .channels { margin-top: 2px; padding-top: 6px; border-top: 1px solid var(--ga-border-subtle); }
      .channels > summary { display: flex; flex-wrap: wrap; align-items: center; gap: var(--ga-field-gap); padding: 2px 0; cursor: pointer; }
      .channels .side { margin: 8px 0 2px; font-size: 11px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-secondary); }
      .channel-row { display: grid; grid-template-columns: max-content minmax(0, 1fr) minmax(0, 1.4fr) minmax(0, 1fr); align-items: center; gap: var(--ga-field-gap); padding: 2px 0; }
      .channel-row > * { min-width: 0; }
      .channel-row .channel-name, .channel-row .daw { font-size: 11px; overflow-wrap: anywhere; }
      .channel-row .daw { color: var(--ga-text-muted); }
      .expose { min-width: 34px; }
      .expose[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .safe[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .lock { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-muted); background: var(--ga-surface-inset); }
      .lock[data-locked] { color: var(--ga-text-inverse); background: var(--ga-accent); }
      .gap[data-tone="good"] { color: var(--ga-accent); }
      .gap[data-tone="off"], .gap[data-tone="stalled"] { color: var(--ga-state-mute); font-weight: 700; }
      .gap[data-tone="idle"] { color: var(--ga-text-muted); }
      .setup { padding: 4px 0; }
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
      .live-row .phase-line { grid-column: 1 / -1; justify-self: start; }
      /* The phase: its setup on a card, what a session made of it, and what a run made of it. The
         tones are the gap's own: accent for lined up, the warning colour for refused. */
      .phase-part { margin-top: 2px; padding-top: 6px; border-top: 1px solid var(--ga-border-subtle); }
      /* Where the DAW can play: a line per output, its label in the card's own label column. */
      .plays { display: grid; grid-template-columns: subgrid; row-gap: 4px; margin-top: 2px; padding-top: 6px; border-top: 1px solid var(--ga-border-subtle); }
      .plays > .head { grid-column: 1 / -1; }
      .play-row { grid-column: 1 / -1; display: grid; grid-template-columns: subgrid; align-items: center; }
      .play-row > .what { grid-column: 2 / -1; display: flex; flex-wrap: wrap; align-items: center; gap: var(--ga-field-gap); min-width: 0; }
      .play-row .readout { overflow-wrap: anywhere; }
      .play-row[data-state="nothing"] .readout { color: var(--ga-state-solo); }
      .plays + .plays { margin-top: 6px; }
      .phase-part > summary { display: flex; flex-wrap: wrap; align-items: center; gap: var(--ga-field-gap); padding: 2px 0; cursor: pointer; }
      .phase-part .field-grid { margin-top: 6px; }
      [data-tone="good"]:is(.phase, .phase-summary, .verdict-text) { color: var(--ga-accent); }
      [data-tone="warn"]:is(.phase, .phase-summary) { color: var(--ga-state-solo); }
      [data-tone="off"]:is(.phase, .phase-summary, .verdict-text), [data-tone="drift"].verdict-text { color: var(--ga-state-mute); font-weight: 700; }
      [data-tone="idle"]:is(.phase, .phase-summary) { color: var(--ga-text-muted); }
      .verdict-row, .phase-row { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 2fr); align-items: center; gap: 10px; padding: 4px 8px; border-radius: 3px; background: var(--ga-surface-raised); }
      .verdict-row + .verdict-row, .phase-row + .phase-row, .witness + .witness { margin-top: 4px; }
      .verdict-row > *, .phase-row > * { min-width: 0; }
      .side-head { margin: 10px 0 4px; font-size: 11px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-secondary); }
      .clean.problem { color: var(--ga-state-mute); font-weight: 700; }
      .reason .hint { grid-column: 2 / -1; font-size: 11px; color: var(--ga-text-muted); }
      .events .gazelle { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-accent-text); background: var(--ga-accent); align-self: center; }
      .events .kind[data-problem] { color: var(--ga-state-mute); }

      /* Phone width: nothing sits side by side, and the long readouts wrap rather than scroll. */
      @media (max-width: 480px) {
        .reason { grid-template-columns: auto minmax(0, 1fr); }
        .reason .fix { grid-column: 1 / -1; justify-self: start; }
        .device-head .name { flex: 1 1 100%; }
        .live-row { grid-template-columns: minmax(0, 1fr) auto; }
        .calibrate-row, .reading, .trim-row { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }
        .verdict-row, .phase-row { grid-template-columns: minmax(0, 1fr); }
        .reason .hint { grid-column: 1 / -1; }
        .events li { flex-wrap: wrap; }
        .channel-row { grid-template-columns: max-content minmax(0, 1fr); }
        .channel-row .field, .channel-row .daw { grid-column: 1 / -1; }
        .play-row > .what { grid-column: 1 / -1; }
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
      h(
        "p",
        { class: "note", "data-testid": "aggregate-names-note" },
        "Each interface is called by its name in Gazelle, and renaming the device renames it here and in the DAW. A DAW shows each channel's name with the interface's name and the channel's number after it, all in 31 characters; with a long name there is no room and the DAW shows the channel's name alone, so a short name for the device in Gazelle keeps that part.",
      ),
    );

    const setup = h("div", { class: "setup field-grid pairs", "data-testid": "aggregate-setup" });
    const exportPath = h("code", { "data-testid": "aggregate-export-path" });
    // The rate the aggregate will actually run at, and where it comes from, under the setup's own
    // choice, which with nothing chosen is the rate the interfaces are running at.
    const rateInForce = h("span", { class: "readout", "data-testid": "aggregate-rate-in-force", "data-explain": "aggregate.rate-in-force" });
    const rateNote = h("p", { class: "note", hidden: true }, rateInForce);
    const setupSection = h(
      "ga-section",
      { heading: "Setup", explain: "aggregate.setup" },
      setup,
      rateNote,
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
      this.#showReasons(store, reasons, noReasons, answer);
      this.#showRegistration(registration, command, registerButton, unregisterButton, answer);
      this.#showPlan(plan, live, liveNote, answer, untracked(() => this.#naming(store)));
      this.#showEvents(events, eventsNote, answer);
      matchButton.hidden = answer.devices.length < 2;
      matchButton.title = buffersMatch(answer) ? "Every interface is already on one buffer size" : `Put every interface on ${matchTarget(answer) ?? "one"} samples: ${RESTARTS}`;
    });

    // Each interface's channels are named from its routing: its USB record group for what feeds each
    // input, and its outputs and mix inputs for where each USB playback channel ends up. Each of those
    // groups is read once, as the Routing page reads it, and again only once the device has been
    // away. Nothing is read for an interface Gazelle does not know the model of.
    this.watch(() => {
      for (const named of this.#naming(store)) {
        const groups = readGroups(named.topology);
        const id = named.deviceId;
        if (id !== undefined && groups.length > 0 && store.routesToRead(id, groups)) untracked(() => void store.readRoutes(id, groups));
      }
    });

    // The cards and the setup are rebuilt only when the setup itself, or what the interfaces are
    // called, changes, so a poll landing does not take away a name half typed or a menu half chosen.
    let shown: string | undefined;
    this.watch(() => {
      const config = store.workspace.value?.aggregate;
      noDevices.hidden = (config?.devices ?? []).length > 0;
      // Only the list and the names decide what is built; everything else about the setup is
      // followed by the controls' own effects, so a rate chosen elsewhere does not throw away a name
      // half typed. What Gazelle last knew of each device is the server's, and changes nothing here.
      const names = interfaceNames(config, this.#naming(store));
      const shape = JSON.stringify([(config?.devices ?? []).map(({ known: _known, ...rest }) => rest), names]);
      if (shape === shown) return;
      shown = shape;
      const watch = this.#renew();
      untracked(() => {
        this.#buildDevices(store, devices, config, names, watch);
        this.#buildSetup(store, setup, config, names, watch);
      });
    });

    this.watch(() => {
      const answer = model.answer.value;
      exportPath.textContent = answer?.export_path ?? "";
      const inForce = rateInForceText(answer);
      rateNote.hidden = inForce === undefined;
      rateInForce.textContent = inForce ?? "";
      this.#fillAdd(store, addSelect, addButton, answer);
    });
  }

  /**
   * How every interface and channel on the page is named, read reactively: the answer, the devices
   * Gazelle has, the person's own names in the workspace, and the routing the store has read.
   */
  #naming(store: Store): AggregateNaming {
    const workspace = store.workspace.value;
    return aggregateNaming(workspace?.aggregate, store.aggregate.answer.value, {
      devices: store.devices.value,
      aliases: workspace?.aliases,
      layouts: workspace?.mixers,
      routing: (deviceId, destination) => (store.topology(deviceId) === undefined ? undefined : store.routing(deviceId).destination(destination).value),
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

  #showReasons(store: Store, list: HTMLElement, none: HTMLElement, answer: AggregateAnswer): void {
    none.hidden = answer.reasons.length > 0;
    list.hidden = answer.reasons.length === 0;
    const shown = JSON.stringify(answer.reasons);
    if (shown === this.#reasonsShown) return;
    this.#reasonsShown = shown;
    list.replaceChildren(...answer.reasons.map((reason) => this.#reasonRow(store, reason)));
  }

  #reasonRow(store: Store, reason: AggregateReason): HTMLElement {
    const model = store.aggregate;
    const severity = h(
      "span",
      { class: "severity", "data-severity": reason.severity, "data-testid": `reason-severity-${reason.code}`, "data-explain": "aggregate.severity" },
      reason.severity === "blocking" ? "STOPS IT" : "WORTH KNOWING",
    );
    const text = h("span", { "data-testid": `reason-${reason.code}` }, reason.message);
    const fix = reason.fix === undefined ? undefined : this.#fixButton(model, reason, reason.fix);
    // A reason the page can say more about, and point at the place on the page that answers it.
    const hint = reasonHint(reason);
    const goes = reasonCard(reason, store.workspace.peek()?.aggregate, untracked(() => this.#naming(store))) === undefined
      ? undefined
      : h(
          "button",
          {
            type: "button",
            class: "fix",
            "data-testid": `reason-goto-${reason.code}`,
            "data-explain": "aggregate.reason-goto-phase",
            "on:click": () => {
              const at = reasonCard(reason, store.workspace.peek()?.aggregate, untracked(() => this.#naming(store)));
              if (at !== undefined) this.#openPhase(at);
            },
          },
          "Set up the phase",
        );
    const button = fix ?? goes;
    return h(
      "li",
      { class: "reason", "data-testid": `reason-row-${reason.code}` },
      severity,
      text,
      ...(button === undefined ? [] : [button]),
      ...(hint === undefined ? [] : [h("span", { class: "hint", "data-testid": `reason-hint-${reason.code}` }, hint)]),
    );
  }

  /** Opens one card's phase setup, brings it into view and puts the first picker under the cursor. */
  #openPhase(index: number): void {
    const part = this.root.querySelector<HTMLDetailsElement>(`[data-testid="device-${index}-phase-part"]`);
    if (part === null || part.hidden) return;
    part.open = true;
    part.scrollIntoView({ block: "center" });
    part.querySelector<HTMLSelectElement>("select:not([disabled])")?.focus();
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

  #buildDevices(store: Store, into: HTMLElement, config: Aggregate | undefined, names: string[], watch: (fn: () => void) => void): void {
    const list = config?.devices ?? [];
    into.replaceChildren(...list.map((device, index) => this.#deviceCard(store, device, index, list.length, names[index] ?? interfaceName(undefined, index, device), watch)));
  }

  #deviceCard(store: Store, device: AggregateDevice, index: number, total: number, named: string, watch: (fn: () => void) => void): HTMLElement {
    const model = store.aggregate;
    const testid = `device-${index}`;

    // Gazelle's name for the device, which is the one name this interface has. It is renamed where
    // the device is, on the Devices and Workspace pages, and there is nothing here to keep in step with it.
    const name = h("span", { class: "name", title: "Its name in Gazelle. Rename the device to rename it here and in the DAW.", "data-testid": `${testid}-name`, "data-explain": "aggregate.device-name" }, named);

    const up = h("button", { type: "button", class: "order", "data-testid": `${testid}-up`, "data-explain": "aggregate.device-up", "aria-label": `Move ${named} earlier`, title: "Earlier: its channels come before the others", "on:click": () => this.#move(store, index, -1) }, "↑");
    const down = h("button", { type: "button", class: "order", "data-testid": `${testid}-down`, "data-explain": "aggregate.device-down", "aria-label": `Move ${named} later`, title: "Later: its channels come after the others", "on:click": () => this.#move(store, index, 1) }, "↓");
    up.disabled = index === 0;
    down.disabled = index === total - 1;
    const remove = h("button", { type: "button", "data-testid": `${testid}-remove`, "data-explain": "aggregate.device-remove", "aria-label": `Take ${named} out of the aggregate` }, "Remove");
    this.onDisconnect(bindConfirm(remove, "Remove", () => this.#removeDevice(store, index)));

    // The vendor driver's own name, once, as a detail: it is what the aggregate opens, not a name.
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
    const phaseNow = h("span", { class: "readout phase", "data-testid": `${testid}-phase`, "data-explain": "aggregate.device-phase" });

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
      "How many channels this interface has is not known until Gazelle knows which device it is, or a DAW opens the aggregate.",
    );
    const channelsNote = h(
      "p",
      { class: "note", "data-testid": `${testid}-channels-note`, hidden: true },
      "Each channel is one of this interface's USB channels: input 1 is what its first USB record channel records, and output 1 is what its first USB playback channel plays. Each is named for what it carries, from Gazelle's routing and your names on the Mixer, so re-routing renames it. A name typed here wins, on this page and in the DAW; clear it and the name from the routing comes back.",
    );
    const channelsCheck = h("p", { class: "note warning", "data-testid": `${testid}-channels-check`, hidden: true });
    const channelsPart = h(
      "details",
      { class: "channels wide", "data-testid": `${testid}-channels-part` },
      h("summary", { "data-testid": `${testid}-channels-open`, "data-explain": "aggregate.device-channels-open" }, h("span", { class: "label" }, "Channels"), channels),
      channelsNote,
      channelsCheck,
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

    const phasePart = this.#phasePart(store, device, index, watch);
    const playsPart = this.#playsPart(store, index, watch);
    const recordsPart = this.#recordsPart(store, index, watch);

    // One grid for the whole card, two label-and-field pairs to a line: what you set down the left,
    // what the interface reports down the right. Every label shares one column, so every value
    // starts at the same place instead of wherever its own little grid happened to put it.
    const field = (label: string, ...value: (Node | string)[]): Node[] => [
      h("span", { class: "label" }, label),
      value.length === 1 && value[0] instanceof HTMLElement ? value[0] : h("span", { class: "field-row" }, ...value),
    ];

    const card = h(
      "div",
      { class: "device-card", "data-testid": `aggregate-${testid}` },
      h("div", { class: "device-head field-row" }, h("span", { class: "label" }, `${index + 1}.`), name, h("span", { class: "spacer" }), up, down, remove),
      h(
        "div",
        { class: "field-grid pairs" },
        ...field("Gazelle device", idSelect, worked),
        ...field("Audio driver", which),
        ...field("Clock", clock, lock),
        ...field("Buffer", bufferMenu, bufferChoice.confirm),
        ...field("Rate", rate),
        ...field("Safe Mode", safe),
        ...field("Gap", gap),
        ...field("Input trim", inTrim),
        ...field("Output trim", outTrim),
        ...field("Phase now", phaseNow),
        channelsPart,
        playsPart,
        recordsPart,
        phasePart,
      ),
      notMatched,
    );

    watch(() => {
      // Which device this is, as the answer resolved it, and the menu of the connected ones with
      // whatever the setup names kept even when it is away.
      const view = viewFor(model.answer.value, index);
      resolvedId = resolvedDeviceId(view, device);
      const how = matchedBy(view);
      const attached = store.devices.value;
      const options = [h("option", { value: "" }, "Work it out")];
      for (const found of attached) options.push(h("option", { value: found.id }, `${displayName(found, store.workspace.value)} (${found.id})`));
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
      const view = viewFor(model.answer.value, index);
      const report = view?.report;
      clock.textContent = report?.clock?.source ?? (report?.attached === true ? "Not reported" : "Not connected");
      lock.toggleAttribute("data-locked", report?.clock?.locked === true);
      lock.textContent = report?.clock?.locked === true ? "LOCK" : "NO LOCK";
      rate.textContent = report?.clock?.hz === undefined ? "Not known" : `${Number((report.clock.hz / 1000).toFixed(3))} kHz`;
      const reading = gapFor(view);
      gap.textContent = reading.text;
      gap.setAttribute("data-tone", reading.tone);
      const phase = cardPhaseView(view?.live, masterIndex(store.workspace.value?.aggregate, model.answer.value) === index);
      phaseNow.textContent = phase.text;
      phaseNow.setAttribute("data-tone", phase.tone);
    });

    watch(() => {
      // The sizes the driver offers, read as the Devices page reads them; its own reading wins.
      // The device is the one the answer resolved, so this is live without anybody choosing one.
      const view = viewFor(model.answer.value, index);
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
      for (const field of [inTrim, outTrim]) field.disabled = !store.connected.value;
    });

    // The channel rows are rebuilt only when how many there are, or what they are called, changes:
    // a poll landing every second must not take away a label half typed.
    let built: string | undefined;
    watch(() => {
      const naming = this.#naming(store)[index];
      const counts = channelCounts(naming);
      channels.textContent = channelSummary(device, counts);
      channels.title = device.inputs === undefined && device.outputs === undefined ? "Every channel the interface has" : "Only the channels the workspace names";
      const known = counts.inputs !== undefined || counts.outputs !== undefined;
      unknownChannels.hidden = known;
      channelsNote.hidden = !known;
      const check = countCheck(naming);
      channelsCheck.hidden = check === undefined;
      channelsCheck.textContent = check ?? "";
      const texts = (input: boolean, count: number | undefined) => Array.from({ length: count ?? 0 }, (_, channel) => channelName(device, naming, input, channel));
      const shape = JSON.stringify([counts, named, naming?.dawName, texts(true, counts.inputs), texts(false, counts.outputs)]);
      if (shape !== built) {
        built = shape;
        untracked(() => {
          inputRows.replaceChildren(...this.#channelRows(store, device, index, named, true, counts.inputs, naming));
          outputRows.replaceChildren(...this.#channelRows(store, device, index, named, false, counts.outputs, naming));
        });
      }
      const connected = store.connected.value;
      for (const control of channelsPart.querySelectorAll<HTMLButtonElement | HTMLInputElement>("button.expose, input.field")) control.disabled = !connected;
    });

    return card;
  }

  /**
   * A follower's phase setup: the channel the cable leaves the callback master on, the one it
   * arrives on here, and a way to clear it, with what the setting means now and the routing it
   * needs. Not offered on the callback master's card, which the others are measured against; a
   * setting left on it from before is shown only so it can be cleared, because the driver refuses it.
   *
   * Nothing is written until both channels are chosen, since half a path is refused: the first pick
   * waits in this tab's view state, so a poll or a rebuild does not take it away.
   */
  #phasePart(store: Store, device: AggregateDevice, index: number, watch: (fn: () => void) => void): HTMLElement {
    const model = store.aggregate;
    const testid = `device-${index}`;
    const setting = phaseSetting(device);
    const draft = store.view<PhasePicks | undefined>(`draft:aggregate:${index}:phase`, undefined);

    const summary = h("span", { class: "readout phase-summary", "data-testid": `${testid}-phase-summary`, "data-explain": "aggregate.device-phase-summary" });
    const note = h("p", { class: "note", "data-testid": `${testid}-phase-note` });
    const leaves = h("select", { "aria-label": "Channel the cable leaves the callback master on", "data-testid": `${testid}-phase-leaves`, "data-no-wheel": true, "data-explain": "aggregate.device-phase-leaves" });
    const arrives = h("select", { "aria-label": "Channel the cable arrives on at this interface", "data-testid": `${testid}-phase-arrives`, "data-no-wheel": true, "data-explain": "aggregate.device-phase-arrives" });
    const clear = h("button", { type: "button", "data-testid": `${testid}-phase-clear`, "data-explain": "aggregate.device-phase-clear" }, "Clear");
    this.onDisconnect(
      bindConfirm(clear, "Clear", () => {
        draft.value = undefined;
        if (phaseSetting(device) !== undefined) this.#editDevice(store, index, (current) => withPhase(current, undefined));
      }),
    );
    const routing = h("p", { class: "note", "data-testid": `${testid}-phase-routing` });
    const pickers = h("div", { class: "field-grid" }, h("span", { class: "label" }, "Leaves the callback master on"), leaves, h("span", { class: "label" }, "Arrives on"), arrives);

    const pick = (key: keyof PhasePicks, value: string) => {
      const chosen = value === "" ? undefined : Number(value);
      const { [key]: _was, ...others } = phasePicks(setting, draft.peek());
      const picks: PhasePicks = chosen === undefined ? others : { ...others, [key]: chosen };
      const next = phaseFromPicks(setting, picks);
      if (next === undefined) {
        draft.value = picks;
        return;
      }
      draft.value = undefined;
      if (next !== setting) this.#editDevice(store, index, (current) => withPhase(current, phaseFromPicks(phaseSetting(current), picks)));
    };
    leaves.addEventListener("change", () => pick("master_output", leaves.value));
    arrives.addEventListener("change", () => pick("input", arrives.value));

    const part = h(
      "details",
      { class: "phase-part wide", "data-testid": `${testid}-phase-part` },
      h("summary", { "data-testid": `${testid}-phase-open`, "data-explain": "aggregate.device-phase-open" }, h("span", { class: "label" }, "Phase setup"), summary),
      note,
      pickers,
      h("div", { class: "add" }, clear),
      routing,
    );
    const opened = store.view<boolean>(`aggregate:${index}:phase-open`, false);
    part.open = opened.peek();
    part.addEventListener("toggle", () => {
      opened.value = part.open;
    });

    const fill = (select: HTMLSelectElement, choices: PhaseChoice[] | undefined, chosen: number | undefined, named: string, called: (channel: number) => string) => {
      const listed = choicesWith(choices, chosen, called);
      const empty = choices === undefined ? `Not known until Gazelle knows which device ${named} is` : "Choose a channel";
      const options = [{ value: "", text: empty }, ...listed.map((choice) => ({ value: String(choice.value), text: choice.text }))];
      const shape = JSON.stringify(options);
      if (select.dataset["shape"] !== shape) {
        select.dataset["shape"] = shape;
        select.replaceChildren(...options.map((option) => h("option", { value: option.value }, option.text)));
      }
      select.value = chosen === undefined ? "" : String(chosen);
    };

    watch(() => {
      const config = store.workspace.value?.aggregate;
      const answer = model.answer.value;
      const naming = this.#naming(store);
      const at = masterIndex(config, answer);
      const isMaster = at === index;
      const masterDevice = at === undefined ? undefined : config?.devices?.[at];
      const master = masterDevice === undefined || at === undefined ? "the callback master" : interfaceName(naming, at, masterDevice);
      const own = interfaceName(naming, index, device);
      const view = phaseSetupView(device, isMaster, master);
      part.hidden = isMaster && setting === undefined;
      summary.textContent = view.summary;
      summary.setAttribute("data-tone", view.tone);
      note.textContent = view.note;
      pickers.hidden = isMaster;
      routing.hidden = isMaster;

      const picks = phasePicks(setting, draft.value);
      const choices = phaseChoices(config, answer, naming, index);
      fill(leaves, choices?.outputs, picks.master_output, master, (channel) => channelName(masterDevice, at === undefined ? undefined : naming[at], false, channel).text);
      fill(arrives, choices?.inputs, picks.input, own, (channel) => channelName(device, naming[index], true, channel).text);
      clear.hidden = setting === undefined && picks.master_output === undefined && picks.input === undefined;

      const ownId = resolvedDeviceId(viewFor(answer, index), device);
      const from = masterDevice === undefined || at === undefined ? undefined : resolvedDeviceId(viewFor(answer, at), masterDevice);
      routing.textContent = phaseRoutingNote(master, own, cablePort(store.workspace.value?.cables, from, ownId));
      const connected = store.connected.value;
      for (const control of [leaves, arrives, clear]) control.disabled = !connected;
    });

    return part;
  }

  /**
   * Where the DAW can play on this interface: a line per hardware output, in the order outputs are
   * named by, saying which USB playback channels reach it and how, and for an output nothing from
   * the DAW reaches, a button that sends it the first free run.
   */
  #playsPart(store: Store, index: number, watch: (fn: () => void) => void): HTMLElement {
    return this.#routeList(store, index, watch, {
      heading: "Where the DAW can play",
      part: "plays",
      row: "play",
      explainHead: "aggregate.plays",
      explainLine: "aggregate.play-line",
      explainSend: "aggregate.play-send",
      lines: playbackOutputs,
    });
  }

  /**
   * Where the DAW can record on this interface: a line per input socket, in the device's own order,
   * saying which USB record channels carry it and how, and for an input nothing records, a button
   * that records it on the first free USB record channels.
   */
  #recordsPart(store: Store, index: number, watch: (fn: () => void) => void): HTMLElement {
    return this.#routeList(store, index, watch, {
      heading: "Where the DAW can record",
      part: "records",
      row: "record",
      explainHead: "aggregate.records",
      explainLine: "aggregate.record-line",
      explainSend: "aggregate.record-send",
      lines: recordingInputs,
    });
  }

  /**
   * One of the two routing lists on a card: a line each, its label in the card's own label column,
   * what it says, and where there is one, the button that puts it right. The button is the app's two
   * click confirm and writes through the store's routing model, exactly as a change on the Routing
   * page does, so the write, the dry run's bytes line and every name that follows from it all follow
   * as they do there. Nothing here writes by itself.
   *
   * The lines are rebuilt only when what they say changes, so a poll landing does not disarm a
   * Confirm half pressed.
   */
  #routeList(
    store: Store,
    index: number,
    watch: (fn: () => void) => void,
    list: {
      heading: string;
      part: string;
      row: string;
      explainHead: string;
      explainLine: string;
      explainSend: string;
      lines: (naming: InterfaceNaming | undefined) => { label: string; state: string; text: string; send?: PlaybackSend; noSend?: string }[];
    },
  ): HTMLElement {
    const testid = `device-${index}`;
    const part = h("div", { class: "plays wide", "data-testid": `${testid}-${list.part}` }, h("span", { class: "label head", "data-explain": list.explainHead }, list.heading));
    let shown: string | undefined;
    let disarms: (() => void)[] = [];
    this.onDisconnect(() => {
      for (const disarm of disarms) disarm();
    });
    watch(() => {
      const naming = this.#naming(store)[index];
      const lines = list.lines(naming);
      const shape = JSON.stringify([naming?.deviceId, lines]);
      part.hidden = lines.length === 0;
      if (shape !== shown) {
        shown = shape;
        for (const disarm of disarms) disarm();
        disarms = [];
        const rows = lines.map((line, at) => {
          const what = h("span", { class: "what" }, h("span", { class: "readout", "data-testid": `${testid}-${list.row}-${at}-text`, "data-explain": list.explainLine }, line.text));
          const send = line.send;
          const id = naming?.deviceId;
          if (send !== undefined && id !== undefined) {
            const button = h("button", { type: "button", class: "fix", title: send.title, "data-testid": `${testid}-${list.row}-${at}-send`, "data-explain": list.explainSend }, send.label);
            disarms.push(bindConfirm(button, send.label, () => void store.routing(id).routeMany(send.destination, send.changes)));
            what.append(button);
          } else if (line.noSend !== undefined) {
            what.append(h("span", { class: "note", "data-testid": `${testid}-${list.row}-${at}-no-send` }, line.noSend));
          }
          return h("div", { class: "play-row", "data-state": line.state, "data-testid": `${testid}-${list.row}-${at}` }, h("span", { class: "label" }, line.label), what);
        });
        untracked(() => part.replaceChildren(part.firstChild as Node, ...rows));
      }
      const connected = store.connected.value;
      for (const button of part.querySelectorAll<HTMLButtonElement>("button")) button.disabled = !connected;
    });
    return part;
  }

  /**
   * One side of an interface's channels: a heading and a row each, or nothing at all when how many
   * there are is not known. A count that is not known offers nothing rather than guessing at one.
   */
  #channelRows(store: Store, device: AggregateDevice, index: number, named: string, input: boolean, count: number | undefined, naming: InterfaceNaming | undefined): Node[] {
    if (count === undefined) return [];
    const group = input ? usbGroups(naming?.topology)?.record : usbGroups(naming?.topology)?.playback;
    const heading = h("p", { class: "side" }, input ? "INPUTS" : "OUTPUTS", group === undefined ? "" : `, ${group.name}`);
    return [heading, ...Array.from({ length: count }, (_, channel) => this.#channelRow(store, device, index, named, input, channel, count, naming))];
  }

  /**
   * One channel: whether the aggregate exposes it, its name, a field for a name of the person's own,
   * and what a DAW will show for it where that says something the name does not. The automatic name
   * is said once: the field is empty until somebody types, and the DAW line is left out when it would
   * only repeat the name.
   */
  #channelRow(store: Store, device: AggregateDevice, index: number, named: string, input: boolean, channel: number, count: number, naming: InterfaceNaming | undefined): HTMLElement {
    const side = input ? "in" : "out";
    const testid = `device-${index}-${side}-${channel}`;
    const listKey = input ? "inputs" : "outputs";
    const nameKey = input ? "input_names" : "output_names";
    const called = channelName(device, naming, input, channel);
    const exposed = isExposed(device[listKey], channel);

    const expose = h(
      "button",
      {
        type: "button",
        class: "expose",
        "aria-pressed": String(exposed),
        "aria-label": `Expose ${called.text}`,
        title: exposed ? `${called.text} is one of the channels a DAW sees` : `${called.text} is kept out of what a DAW sees`,
        "data-testid": `${testid}-expose`,
        "data-explain": "aggregate.channel-expose",
        "on:click": () => this.#editDevice(store, index, (current) => withField(current, listKey, withChannelExposed(current[listKey] as number[] | undefined, count, channel, !exposed))),
      },
      exposed ? "On" : "Off",
    );

    const label = h("input", {
      class: "field",
      maxlength: String(CHANNEL_LABEL_MAX),
      placeholder: "Your name for it",
      "aria-label": `Name of your own for ${called.usb}`,
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

    // The DAW's reference is the device's DAW name, its model's short form where nobody named it.
    const daw = dawLine(naming?.dawName ?? named, channel, called);
    return h(
      "div",
      { class: "channel-row", "data-testid": testid },
      expose,
      h("span", { class: "channel-name readout", "data-testid": `${testid}-name`, "data-explain": "aggregate.channel-name" }, called.text),
      label,
      ...(daw === undefined ? [] : [h("span", { class: "daw readout", "data-testid": `${testid}-daw`, "data-explain": "aggregate.channel-daw" }, `In a DAW: ${daw}`)]),
    );
  }

  // -------------------------------------------------------------------------------------------
  // Editing the setup
  // -------------------------------------------------------------------------------------------

  #buildSetup(store: Store, into: HTMLElement, config: Aggregate | undefined, names: string[], watch: (fn: () => void) => void): void {
    const devices = config?.devices ?? [];
    // Each interface by its name, and written as its registry key, which a rename cannot change,
    // so renaming the device in Gazelle never loses the master.
    const master = h("select", { "aria-label": "Which interface drives the callback", "data-testid": "aggregate-master", "data-no-wheel": true, "data-explain": "aggregate.master" });
    master.replaceChildren(
      h("option", { value: "" }, "The first one"),
      ...devices.flatMap((device, index) => {
        const reference = masterReference(device);
        return reference === undefined ? [] : [h("option", { value: reference }, names[index] ?? reference)];
      }),
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
      const named = typeof current?.callback_master === "string" && current.callback_master.trim() !== "";
      const at = named ? masterIndex(current) : undefined;
      const chosen = at === undefined ? undefined : devices[at];
      master.value = chosen === undefined ? "" : (masterReference(chosen) ?? "");
      alignment.value = current?.alignment === "lowest_latency" ? "lowest_latency" : "aligned";
      rate.value = typeof current?.rate === "number" && RATE_HZ.includes(current.rate) ? String(current.rate) : "";
      buffer.value = typeof current?.buffer_size === "number" && BUFFER_SIZES.includes(current.buffer_size) ? String(current.buffer_size) : "";
      for (const control of [master, alignment, rate, buffer]) control.disabled = !store.connected.value;
    });
  }

  #fillAdd(store: Store, select: HTMLSelectElement, add: HTMLButtonElement, answer: AggregateAnswer | undefined): void {
    const workspace = store.workspace.peek();
    const already = new Set((workspace?.aggregate?.devices ?? []).map((device) => (device.key ?? "").toLowerCase()));
    const offered = (answer?.drivers ?? []).filter((entry) => !entry.is_aggregate && !already.has(entry.key.toLowerCase()));
    const devices = store.devices.value;
    select.replaceChildren(
      ...(offered.length === 0
        ? [h("option", { value: "" }, "No other audio driver on this PC")]
        : offered.map((entry) => h("option", { value: entry.key }, driverOfferText({ key: entry.key, ...(entry.description === undefined ? {} : { description: entry.description }) }, devices, workspace?.aliases)))),
    );
    select.disabled = offered.length === 0;
    add.disabled = offered.length === 0;
  }

  /** Adds a driver by its key alone: what it is called is Gazelle's name for the device it turns out to be. */
  #addDevice(store: Store, key: string): void {
    if (key === "") return;
    store.editAggregate((current) => ({ ...current, devices: [...(current.devices ?? []), { key }] }));
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
    const naming = (): AggregateNaming => untracked(() => this.#naming(store));
    const picksNow = (): CalibratePicks => reconcilePicks(chosen.peek(), store.workspace.peek()?.aggregate, naming());
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
    // Another pass or another reference is other cabling, so it starts from its own defaults.
    const setup = () => store.workspace.peek()?.aggregate;
    direction.addEventListener("change", () => change(withPass(picksNow(), setup(), naming(), direction.value === "outputs" ? "outputs" : "inputs", picksNow().reference)));

    const reference = menu("Which interface every cable has an end on", "calibrate-reference", "aggregate.calibrate-reference");
    reference.addEventListener("change", () => change(withPass(picksNow(), setup(), naming(), picksNow().direction, reference.value)));

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
        const request = calibrateRequest(picksNow(), store.workspace.peek()?.aggregate, naming());
        if (request !== undefined) void model.startCalibration(request);
      }),
    );
    const check = h("button", { type: "button", "data-testid": "calibrate-check", "data-explain": "aggregate.calibrate-check" }, "Check");
    this.onDisconnect(
      bindConfirm(check, "Check", () => {
        const request = calibrateRequest(picksNow(), store.workspace.peek()?.aggregate, naming(), { check: true });
        if (request !== undefined) void model.startCalibration(request);
      }),
    );
    const stop = h("button", { type: "button", "data-testid": "calibrate-stop", "data-explain": "aggregate.calibrate-stop", hidden: true, "on:click": () => void model.stopCalibration() }, "Stop");
    const step = h("span", { class: "readout", "data-testid": "calibrate-step", "data-explain": "aggregate.calibrate-step" });
    const fill = h("span", { class: "fill" });
    const track = h("span", { class: "track" }, fill);
    const running = h("div", { class: "running", "data-testid": "calibrate-running", hidden: true }, step, track);

    const summary = h("p", { class: "note", "data-testid": "calibrate-summary", hidden: true });
    const cleanText = h("span", { class: "readout", "data-explain": "aggregate.calibrate-clean" });
    const clean = h("p", { class: "note clean", "data-testid": "calibrate-clean", hidden: true }, cleanText);
    const phaseRefused = h("p", { class: "drift", role: "alert", "data-testid": "calibrate-phase-refused", hidden: true });
    const drift = h("p", { class: "drift", role: "alert", "data-testid": "calibrate-drift", hidden: true });
    const readings = h("div", { "data-testid": "calibrate-readings" });
    const phases = h("div", { "data-testid": "calibrate-phases" });
    const witnesses = h("div", { "data-testid": "calibrate-witnesses" });
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
      h(
        "p",
        { class: "note", "data-testid": "calibrate-check-note" },
        "Measure finds the trims, and the phase reference written with each, with nothing lined up; Check lines the session up exactly as a DAW's is and says how far apart a recording would land now, and writes nothing.",
      ),
      h("div", { class: "setup field-grid pairs", "data-testid": "calibrate-setup" }, h("span", { class: "label" }, "Pass"), direction, h("span", { class: "label" }, "Reference"), reference, h("span", { class: "label" }, "Clicks"), clicks, h("span", { class: "label" }, "Level"), level),
      rows,
      h("p", { class: "note", "data-testid": "calibrate-patch-note" }, "Patch it like this, one cable a line, then press Measure or Check, each twice:"),
      cables,
      h(
        "p",
        { class: "note", "data-testid": "calibrate-routing-note" },
        "Every channel here is one of the interfaces' USB channels, so the click only gets there if their own routing carries it: on the interface that plays it, route each USB playback channel chosen above to the output socket its cable leaves from, and on each interface that records it, route the input socket its cable arrives at to the USB record channel chosen above. The phase needs the same of its own path, the one set under Phase setup on each card: the callback master's USB playback channel to its S/PDIF output, and the follower's S/PDIF input to its USB record channel. On a fresh setup neither path is there, and it reads as nothing heard. Both are on the Routing page.",
      ),
      h(
        "p",
        { class: "note warning", "data-testid": "calibrate-warning" },
        "It plays a click out of a real output at the level above, so turn monitors down first, and it takes both audio drivers for itself while it runs, so close any DAW that has them open. Measure and Check each ask for a confirming click.",
      ),
      problem,
      h("div", { class: "add" }, measure, check, stop),
      running,
      refusal,
      summary,
      clean,
      phaseRefused,
      drift,
      readings,
      phases,
      witnesses,
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
      const named = this.#naming(store);
      const picks = reconcilePicks(chosen.value, config, named);
      const devices = calibrateDevices(config, named);
      const outputs = interfaceChannels(config, named, false);
      const inputs = interfaceChannels(config, named, true);

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
        ...cablingSteps(picks, config, named).map((cable, at) =>
          h("li", { "data-testid": `calibrate-cable-${at}` }, h("span", { class: "at" }, `${at + 1}.`), h("span", { class: "cable readout", "data-explain": "aggregate.calibrate-cable" }, cable.text)),
        ),
      );

      const why = calibrateProblem(picks, config, named);
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
      check.hidden = isRunning;
      measure.disabled = why !== undefined || offered === false || !store.connected.value || model.busy.value !== undefined;
      check.disabled = measure.disabled;
      for (const control of [direction, reference, clicks, level, ...controls.flatMap(({ output, input }) => [output, input])]) control.disabled = isRunning || !store.connected.value;

      const said = model.calibrationProblem.value ?? (state?.state === "failed" ? state.refusal : undefined);
      refusal.hidden = said === undefined;
      refusal.textContent = said ?? "";

      // What it measured, rebuilt only when the outcome itself changes.
      // A run names each interface by its DAW name; the page goes by Gazelle's.
      const outcome = withPageNames(state?.state === "done" ? state.outcome : undefined, named);
      const shown = JSON.stringify(outcome ?? null);
      if (shown !== shownOutcome) {
        shownOutcome = shown;
        const checking = outcome?.checking === true;
        summary.hidden = outcome === undefined;
        summary.textContent = outcome === undefined ? "" : outcomeSummary(outcome);
        const wasClean = runCleanText(outcome);
        clean.hidden = wasClean === undefined;
        clean.classList.toggle("problem", wasClean?.problem === true);
        clean.setAttribute("role", wasClean?.problem === true ? "alert" : "status");
        cleanText.textContent = wasClean?.text ?? "";
        const refused = phaseRefusedText(outcome);
        phaseRefused.hidden = refused === undefined;
        phaseRefused.textContent = refused ?? "";
        drift.hidden = !driftFound(outcome);
        drift.textContent = driftFound(outcome) ? "The interfaces are not sharing one clock. A trim cannot answer that: it would be right now and wrong in a minute. Put every interface on the clock that comes down the digital cable, then measure again." : "";
        // A check is a verdict per interface and offers nothing; a measurement is readings and trims.
        readings.replaceChildren(
          ...(checking
            ? checkVerdicts(outcome).map((verdict) => this.#verdictRow(verdict))
            : (outcome?.readings ?? []).map((reading) => this.#readingRow(reading))),
        );
        const phaseViews = runPhaseViews(outcome);
        phases.replaceChildren(...(phaseViews.length === 0 ? [] : [h("p", { class: "side-head" }, "THE PHASE"), ...phaseViews.map((phase) => this.#runPhaseRow(phase))]));
        const witnessed = witnessViews(outcome, inputs, config, named);
        witnesses.replaceChildren(...(witnessed.length === 0 ? [] : [h("p", { class: "side-head" }, "LISTENED IN ON"), ...witnessed.map((witness, at) => this.#witnessRow(witness, at))]));
        trims.replaceChildren(...(checking ? [] : trimRows(outcome).map((trim, at) => this.#trimRow(trim, at))));
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
    outputs: ReturnType<typeof interfaceChannels>,
    inputs: ReturnType<typeof interfaceChannels>,
  ): { row: HTMLElement; output: HTMLSelectElement; input: HTMLSelectElement } {
    const pick = (side: "outputs" | "inputs", list: ReturnType<typeof interfaceChannels>, label: string, explain: string) => {
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
          : offered.map((channel) => h("option", { value: String(channel.channel) }, channel.text))),
      );
      return select;
    };
    const output = pick("outputs", outputs, "Plays the click", "aggregate.calibrate-plays");
    const input = pick("inputs", inputs, "Records the click", "aggregate.calibrate-records");
    const row = h(
      "div",
      { class: "calibrate-row field-line", "data-testid": `calibrate-row-${at}` },
      h("span", { class: "who" }, device),
      h("span", { class: "field-row" }, h("span", { class: "label" }, "Plays"), output),
      h("span", { class: "field-row" }, h("span", { class: "label" }, "Records"), input),
    );
    return { row, output, input };
  }

  /** One interface's reading: how far out, how steady, and how much it had to go on. */
  #readingRow(reading: Parameters<typeof readingView>[0]): HTMLElement {
    const view = readingView(reading);
    const row = h(
      "div",
      { class: "reading field-line", "data-testid": `calibrate-reading-${view.device}` },
      h("span", {}, view.device, reading.is_reference ? " (reference)" : ""),
      h("span", { class: "readout lag", "data-tone": view.tone, "data-testid": `calibrate-lag-${view.device}`, "data-explain": "aggregate.calibrate-lag" }, view.lag),
      h("span", { class: "readout", "data-testid": `calibrate-spread-${view.device}`, "data-explain": "aggregate.calibrate-spread" }, view.spread),
      h("span", { class: "readout", "data-testid": `calibrate-clicks-${view.device}`, "data-explain": "aggregate.calibrate-found" }, view.clicks),
    );
    const after: Node[] = [];
    // The server's own sentence for this interface, and the drift finding, which is the serious one.
    if (view.note !== undefined) after.push(h("p", { class: "note", "data-testid": `calibrate-note-${view.device}` }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-note" }, view.note)));
    if (view.drift !== undefined) after.push(h("p", { class: "drift", "data-testid": `calibrate-drift-${view.device}`, "data-explain": "aggregate.calibrate-drift" }, view.drift));
    if (view.lost !== undefined) after.push(h("p", { class: "drift", "data-testid": `calibrate-lost-${view.device}` }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-lost" }, view.lost)));
    return after.length === 0 ? row : h("div", {}, row, ...after);
  }

  /** One interface's verdict from a check: how far apart a recording would land now. */
  #verdictRow(verdict: VerdictView): HTMLElement {
    const row = h(
      "div",
      { class: "verdict-row field-line", "data-testid": `calibrate-verdict-${verdict.device}` },
      h("span", {}, verdict.device),
      h("span", { class: "readout verdict-text", "data-tone": verdict.tone, "data-testid": `calibrate-verdict-text-${verdict.device}`, "data-explain": "aggregate.calibrate-verdict" }, verdict.text),
    );
    if (verdict.note === undefined) return row;
    return h("div", {}, row, h("p", { class: "note", "data-testid": `calibrate-note-${verdict.device}` }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-note" }, verdict.note)));
  }

  /** What the driver made of one interface's phase at the start of the run. */
  #runPhaseRow(phase: RunPhaseView): HTMLElement {
    const row = h(
      "div",
      { class: "phase-row field-line", "data-testid": `calibrate-phase-${phase.device}` },
      h("span", {}, phase.device),
      h(
        "span",
        { class: "field-row" },
        h("span", { class: "readout phase", "data-tone": phase.tone, "data-testid": `calibrate-phase-state-${phase.device}`, "data-explain": "aggregate.calibrate-phase" }, phase.text),
        ...(phase.figures === undefined ? [] : [h("span", { class: "readout", "data-testid": `calibrate-phase-figures-${phase.device}`, "data-explain": "aggregate.calibrate-phase-figures" }, phase.figures)]),
      ),
    );
    if (phase.note === undefined) return row;
    return h("div", {}, row, h("p", { class: "note", "data-testid": `calibrate-phase-note-${phase.device}` }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-phase-note" }, phase.note)));
  }

  /** One extra channel the run listened in on. It changes no trim. */
  #witnessRow(witness: WitnessView, at: number): HTMLElement {
    const row = h(
      "div",
      { class: "reading witness field-line", "data-testid": `calibrate-witness-${at}` },
      h("span", {}, `${witness.channel}, on ${witness.device}`),
      h("span", { class: "readout lag", "data-tone": witness.tone, "data-testid": `calibrate-witness-${at}-lag`, "data-explain": "aggregate.calibrate-witness" }, witness.lag),
      h("span", { class: "readout", "data-testid": `calibrate-witness-${at}-spread`, "data-explain": "aggregate.calibrate-spread" }, witness.spread),
      h("span", { class: "readout", "data-testid": `calibrate-witness-${at}-clicks`, "data-explain": "aggregate.calibrate-found" }, witness.clicks),
    );
    const after: Node[] = [];
    if (witness.note !== undefined) after.push(h("p", { class: "note" }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-note" }, witness.note)));
    if (witness.lost !== undefined) after.push(h("p", { class: "drift" }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-lost" }, witness.lost)));
    return after.length === 0 ? row : h("div", {}, row, ...after);
  }

  /** One trim the measurement implies: what it is now, what was measured, and what it would become. */
  #trimRow(trim: ReturnType<typeof trimRows>[number], at: number): HTMLElement {
    const cell = (label: string, value: string, testid: string, explain: string) =>
      h("span", { class: "field-row" }, h("span", { class: "label" }, label), h("span", { class: "readout", "data-testid": testid, "data-explain": explain }, value));
    const row = h(
      "div",
      { class: "trim-row field-line", "data-testid": `calibrate-trim-${at}` },
      h("span", {}, `${trim.device}, ${trim.what.toLowerCase()}`),
      cell("Now", trim.was, `calibrate-trim-${at}-was`, "aggregate.calibrate-trim-was"),
      cell("Measured", trim.measured, `calibrate-trim-${at}-measured`, "aggregate.calibrate-trim-measured"),
      cell("Would be", trim.now, `calibrate-trim-${at}-now`, "aggregate.calibrate-trim-now"),
    );
    const after: Node[] = [];
    // The phase reference written with this trim, and what writing it does to the one there now.
    if (trim.reference !== undefined) after.push(h("p", { class: "note", "data-testid": `calibrate-trim-${at}-reference` }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-phase-reference" }, trim.reference)));
    // One the server is not offering: it is shown, with its reason, and the button passes it over.
    if (trim.notApplied !== undefined) after.push(h("p", { class: "note", "data-testid": `calibrate-trim-${at}-not-applied` }, h("span", { class: "readout", "data-explain": "aggregate.calibrate-not-applied" }, trim.notApplied)));
    return after.length === 0 ? row : h("div", {}, row, ...after);
  }

  /**
   * Writes the measured trims into the setup, the same way every other field on this page does. A
   * run names each interface by Gazelle's name for it, which is what finds its entry.
   */
  #applyTrims(store: Store): void {
    const naming = untracked(() => this.#naming(store));
    const outcome = withPageNames(store.aggregate.calibration.peek()?.outcome, naming);
    const changing = trimsToApply(outcome);
    store.editAggregate((current) => withMeasuredTrims(current, outcome, interfaceNames(current, naming)));
    store.view<string | undefined>("aggregate:calibrate:applied", undefined).value = appliedTrimsText(changing);
  }

  // -------------------------------------------------------------------------------------------
  // Live, while a DAW has it open
  // -------------------------------------------------------------------------------------------

  #showPlan(fields: HTMLElement, rows: HTMLElement, note: HTMLElement, answer: AggregateAnswer, naming: AggregateNaming): void {
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
        ...row("Master", pageNameOf(plan.master, naming), "aggregate.plan-master", "master"),
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
        const phase = livePhaseView(device);
        return h(
          "div",
          { class: "live-row field-line", "data-testid": `live-${device.name}` },
          h("span", {}, pageNameOf(device.name, naming), device.is_master ? " (master)" : ""),
          h("span", { class: "readout gap", "data-tone": reading.tone, "data-testid": `live-gap-${device.name}`, "data-explain": "aggregate.live-gap" }, reading.text),
          h("span", { class: "readout", "data-testid": `live-callbacks-${device.name}`, "data-explain": "aggregate.live-callbacks" }, `${device.callbacks} blocks`),
          h("span", { class: "readout", "data-testid": `live-dropped-${device.name}`, "data-explain": "aggregate.live-dropped" }, `${device.dropped} dropped`),
          h("span", { class: "readout", "data-testid": `live-starved-${device.name}`, "data-explain": "aggregate.live-starved" }, `${device.starved} starved`),
          ...(phase === undefined
            ? []
            : [
                h(
                  "span",
                  { class: "readout phase phase-line", "data-tone": phase.tone, "data-testid": `live-phase-${device.name}`, "data-explain": "aggregate.live-phase" },
                  phase.figures === undefined ? `Phase: ${phase.text}` : `Phase: ${phase.text}. ${phase.figures}`,
                ),
              ]),
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
      ...answer.events.map((event) => {
        const view = eventView(event);
        return h(
          "li",
          { "data-testid": "aggregate-event", ...(view.gazelle ? { "data-gazelle": "" } : {}) },
          h("span", { class: "at" }, view.at),
          h("span", { class: "kind", "data-testid": "aggregate-event-kind", "data-explain": "aggregate.event-kind", ...(view.problem ? { "data-problem": "" } : {}) }, view.kind),
          ...(view.gazelle ? [h("span", { class: "gazelle", "data-testid": "aggregate-event-gazelle", "data-explain": "aggregate.event-gazelle" }, "GAZELLE")] : []),
          h("span", { "data-testid": "aggregate-event-message" }, view.message),
        );
      }),
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
