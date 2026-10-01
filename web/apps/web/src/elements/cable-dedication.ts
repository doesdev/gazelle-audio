// A declared cable's "Dedicated to phase and clock" setting, on the Workspace page's cable rows.
//
// Offered only where it can mean something (store/phase-path.ts says where), and otherwise shown
// greyed with the reason. Turning it on reads the routing it needs, works the plan out and shows
// every change in a confirm before anything is written; turning it off asks whether to keep the
// follower's phase setup. While it is on, the row says what the cable keeps, says so when the
// dedication no longer means anything, and offers the follower's clock fix when its clock does not
// follow the cable, behind a confirm of its own, because a clock change interrupts the audio.

import { h } from "../core/dom.ts";
import { effect } from "../core/signal.ts";
import { usbGroups } from "../store/aggregate.ts";
import { applyDedication, clockAdvice, destinationWords, eligibility, groupsToRead, phasePathContext, planDedication, staleness, storeRouteWriter, withoutDedication } from "../store/phase-path.ts";
import { sourceLabel } from "../store/channels.ts";
import { groupName } from "../store/names.ts";
import type { Cable, Store } from "../store/store.ts";
import { bindConfirm } from "./controls.ts";

export const DEDICATION_STYLES = `
  .dedication { flex: 1 1 100%; display: grid; gap: 6px; padding: 2px 0 4px; }
  .dedication .line { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; }
  .dedication .kept { font-size: 12px; }
  .dedication .why { margin: 0; font-size: 11px; color: var(--ga-text-muted); }
  .dedication ul.changes-list { margin: 0; padding: 0 0 0 18px; list-style: disc; display: grid; gap: 2px; font-size: 11px; }
  .dedication .status { margin: 0; font-size: 11px; color: var(--ga-text-secondary); }
  .dedication button[aria-pressed="true"] { border-color: var(--ga-accent); }
  .dedication button[data-armed] { outline: 2px dashed var(--ga-state-mute); outline-offset: -2px; }
`;

/**
 * One cable's setting. `track` keeps the effects it makes, which the page disposes of when the rows
 * are rebuilt, and the disarms of its two-click buttons.
 */
export function dedicationPart(store: Store, cable: Cable, track: (dispose: () => void) => void): HTMLElement {
  const id = cable.id;
  const part = h("div", { class: "dedication", "data-testid": `cable-dedication-${id}` });
  const toggle = h("button", { type: "button", "aria-pressed": String(cable.dedicated !== undefined), "data-testid": `cable-dedicate-${id}`, "data-explain": "workspace.cable-dedicate" }, cable.dedicated === undefined ? "Dedicate to phase and clock" : "Turn off");
  const why = h("p", { class: "why", "data-testid": `cable-dedicate-why-${id}`, hidden: true });
  const kept = h("span", { class: "kept readout", "data-testid": `cable-dedicated-${id}`, "data-explain": "workspace.cable-dedicated", hidden: true });
  const stale = h("p", { class: "why warn", role: "alert", "data-testid": `cable-dedicated-stale-${id}`, hidden: true });
  const clock = h("button", { type: "button", class: "fix", "data-testid": `cable-clock-${id}`, "data-explain": "workspace.cable-clock", hidden: true });
  const clockNote = h("p", { class: "why", "data-testid": `cable-clock-note-${id}`, hidden: true });
  const status = h("p", { class: "status", role: "status", "data-testid": `cable-dedicate-status-${id}`, hidden: true });
  const confirm = h("div", { class: "confirm", role: "group", "aria-label": "Confirm the change to this cable", "data-testid": `cable-dedicate-confirm-${id}`, hidden: true });
  let busy = false;
  // What the last change came to, kept for the tab: the rows are rebuilt when the cables change,
  // which is exactly when there is something to say.
  const said = store.view<{ text: string; problem: boolean } | undefined>(`cable:${id}:dedication`, undefined);
  const say = (text: string | undefined, problem = false) => {
    said.value = text === undefined ? undefined : { text, problem };
  };
  track(
    effect(() => {
      const now = said.value;
      status.hidden = now === undefined;
      status.textContent = now?.text ?? "";
      status.classList.toggle("warn", now?.problem === true);
    }),
  );
  const close = () => {
    confirm.hidden = true;
    confirm.replaceChildren();
  };

  // The clock fix: the same change the Aggregate page's readiness offers, behind a second click.
  let clockTarget: { deviceId: string; index: number } | undefined;
  track(
    bindConfirm(clock, () => clock.dataset["label"] ?? "", () => {
      if (clockTarget === undefined) return;
      try {
        store.setClockSource(clockTarget.deviceId, clockTarget.index);
      } catch (error) {
        say(error instanceof Error ? error.message : String(error), true);
      }
    }),
  );

  const offer = async () => {
    if (busy) return;
    busy = true;
    say("Reading the routing it needs...");
    try {
      const eligible = eligibility(cable, phasePathContext(store));
      if (!eligible.ok) {
        say(eligible.why, true);
        return;
      }
      for (const read of groupsToRead(eligible.roles, phasePathContext(store))) await store.readRoutes(read.deviceId, read.destinations);
      const context = phasePathContext(store);
      const result = planDedication(cable, context);
      if (!result.ok) {
        say(result.why, true);
        return;
      }
      say(undefined);
      const { plan } = result;
      const advice = clockAdvice(cable, context);
      const lines = [...plan.lines];
      if (advice !== undefined) {
        lines.push(`${context.deviceName(advice.deviceId)}'s clock is ${advice.now}, not the ${advice.name} its cable arrives on. That is not changed here: once this is done, "${advice.label}" is offered beside the cable, with a confirm of its own, because a clock change interrupts the audio.`);
      }
      const apply = h("button", { type: "button", "data-testid": `cable-dedicate-apply-${id}`, "data-explain": "workspace.cable-dedicate-apply" }, plan.writes.length === 0 ? "Dedicate" : "Write it and dedicate");
      const cancel = h("button", { type: "button", "data-testid": `cable-dedicate-cancel-${id}`, "data-explain": "workspace.cable-dedicate-cancel" }, "Cancel");
      apply.addEventListener("click", async () => {
        apply.disabled = cancel.disabled = true;
        const done = await applyDedication(plan, { route: storeRouteWriter(store), editWorkspace: (update) => store.editWorkspace(update), deviceName: context.deviceName });
        close();
        say(done.text, !done.ok);
      });
      cancel.addEventListener("click", close);
      confirm.replaceChildren(
        h("p", {}, plan.writes.length === 0 ? "Dedicate this cable to the phase measurement and the clock? The routing it needs is there already, so nothing is written to the interfaces:" : "Dedicate this cable to the phase measurement and the clock? This is written:"),
        h("ul", { class: "changes-list", "data-testid": `cable-dedicate-lines-${id}` }, ...lines.map((line) => h("li", {}, line))),
        h("p", { class: "note" }, "While it is dedicated, the Routing page marks these channels and asks before a change that would break the path, and the Aggregate page says when it is broken."),
        h("div", { class: "actions" }, apply, cancel),
      );
      confirm.hidden = false;
    } finally {
      busy = false;
    }
  };

  const release = () => {
    const keep = h("button", { type: "button", "data-testid": `cable-release-keep-${id}`, "data-explain": "workspace.cable-release-keep" }, "Turn off, keep the phase setup");
    const clear = h("button", { type: "button", "data-testid": `cable-release-clear-${id}`, "data-explain": "workspace.cable-release-clear" }, "Turn off and clear the phase setup");
    const cancel = h("button", { type: "button", "data-testid": `cable-release-cancel-${id}`, "data-explain": "workspace.cable-dedicate-cancel" }, "Cancel");
    const done = (clearPhase: boolean) => {
      close();
      const saved = store.editWorkspace((workspace) => withoutDedication(workspace, id, clearPhase));
      say(saved ? (clearPhase ? "No longer dedicated, and the phase setup is cleared: its two channels are back in your DAW, and nothing is phase measured." : "No longer dedicated. The routing and the phase setup are as they were, so the phase is still measured over it; only the guard is gone.") : "Nothing was changed: Gazelle is not connected.", !saved);
    };
    keep.addEventListener("click", () => done(false));
    clear.addEventListener("click", () => done(true));
    cancel.addEventListener("click", close);
    confirm.replaceChildren(
      h("p", {}, "Turn the dedication off? Nothing is written to the interfaces: the routing stays as it is. Keeping the phase setup keeps the measurement going over the same channels, still hidden from your DAW; clearing it gives both channels back to your DAW and stops the measurement."),
      h("div", { class: "actions" }, keep, clear, cancel),
    );
    confirm.hidden = false;
  };

  toggle.addEventListener("click", () => {
    if (!confirm.hidden) return;
    if (cable.dedicated === undefined) void offer();
    else release();
  });

  track(
    effect(() => {
      const context = phasePathContext(store);
      const connected = store.connected.value;
      if (cable.dedicated === undefined) {
        const eligible = eligibility(cable, context);
        toggle.disabled = !connected || !eligible.ok;
        why.hidden = eligible.ok;
        why.textContent = eligible.ok ? "" : eligible.why;
        toggle.title = eligible.ok ? "Give this cable over to the aggregate's phase measurement and the clock: its routing is written once, after a confirm that lists every change" : eligible.why;
        return;
      }
      toggle.disabled = !connected;
      const dedication = cable.dedicated;
      const master = context.topology(cable.from.device_id);
      const follower = context.topology(cable.to.device_id);
      const play = usbGroups(master);
      const record = usbGroups(follower);
      const out = master === undefined ? undefined : master.outputs.findIndex((group) => group.type === cable.from.port);
      const output = master === undefined || play === undefined ? `USB playback channel ${dedication.phase_output + 1}` : sourceLabel(master, { group: play.playbackPosition, channel: dedication.phase_output });
      const socket = master === undefined || out === undefined || out < 0 ? "the cable" : destinationWords(master, out, cable.from.first);
      const input = record === undefined ? `USB record channel ${dedication.phase_input + 1}` : `${groupName(record.record)} ${dedication.phase_input + 1}`;
      kept.hidden = false;
      kept.textContent = `Dedicated to phase and clock: ${context.deviceName(cable.from.device_id)} ${output} → ${socket} → ${context.deviceName(cable.to.device_id)} ${input}`;
      const staleWhy = staleness(cable, context.workspace, context.deviceName);
      stale.hidden = staleWhy === undefined;
      stale.textContent = staleWhy ?? "";
      const advice = staleWhy === undefined ? clockAdvice(cable, context) : undefined;
      clockTarget = advice;
      clock.hidden = advice === undefined;
      clockNote.hidden = advice === undefined;
      if (advice !== undefined) {
        clock.dataset["label"] = advice.label;
        if (!clock.hasAttribute("data-armed")) clock.textContent = advice.label;
        clock.title = `${advice.label}: a clock change interrupts the audio. Click twice.`;
        clockNote.textContent = `${context.deviceName(advice.deviceId)}'s clock is ${advice.now}, not the ${advice.name} this cable arrives on, so it follows USB once a DAW opens it and the two drift apart.`;
      }
      clock.disabled = !connected;
    }),
  );

  part.append(h("div", { class: "line" }, toggle, kept, clock), why, stale, clockNote, confirm, status);
  return part;
}
