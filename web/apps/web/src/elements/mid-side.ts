// <ga-mid-side device-id="…">: monitoring a mid and a side microphone as stereo, on the Mixer page
// above the channels (store/mid-side.ts has the reasoning). Fetched with the page's first visit
// (lazy.ts), like the mix notice beside it.
//
// - Started from the top bar's "Monitor as M/S" while exactly two channels are selected, the first
//   as the mid and the second as the side, it shows a confirm: the two roles with Swap, every change
//   it would make (both side strips go through free effect chains holding the same effect, one
//   inverting), what has been measured of that effect's delay, and what is recorded. Nothing is sent
//   until Confirm. Without two free chains and two free instances of an inverting effect it says
//   why, and what to free up, and offers nothing else.
// - Each decode then has one line: its name, a Width control (the side strips' level against the
//   mid's, which moves both side strips through their link), what is recorded, and Remove, which
//   lists what it puts back and waits for Confirm too. In a mix the decode does not play in, the
//   line offers to play it there the same way.
// - Whatever would quietly break the decode in the mix shown is listed in the warning colour, as
//   the mix notice lists a stray, with "Put it back" behind the same kind of confirm. A decode saved
//   by an earlier way (a second preamp, or one chain) says it is no longer supported and offers only
//   Remove.

import { h } from "../core/dom.ts";
import { effect, signal, untracked } from "../core/signal.ts";
import { addToMixPlan, decodes, playsIn, problems, removePlan, repairPlan, retired, setupPlan, setWidth, UndoneError, width, WIDTH_RANGE, type Plan, type Planned } from "../store/mid-side.ts";
import { bindControl, TOUCH_RISE_PER_S } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";

/** What the confirm is open for: set by the top bar's button and a group band's ×, and by this element's own buttons. */
export type MidSideAsk = { kind: "setup"; mid: string; side: string } | { kind: "add" | "remove" | "repair"; group: string };

const widthText = (value: number) => `${value > 0 ? "+" : ""}${value} dB`;

export class GaMidSide extends GaElement {
  static override styles = [
    sheet(`
      :host { display: grid; gap: 6px; }
      :host([hidden]) { display: none; }
      .rows { display: grid; gap: 6px; }
      .rows:empty { display: none; }
      .row, .confirm { display: grid; gap: 6px; padding: 6px 10px; border: 1px solid var(--ga-border-strong); border-left-width: 4px; border-radius: 3px; background: var(--ga-surface-raised); font-size: 12px; line-height: 1.4; }
      .row[data-warning] { border-color: var(--ga-notice-warning); }
      .confirm { border-color: var(--ga-accent); }
      .line { --ga-field-height: 26px; display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; }
      .title { font-weight: 600; }
      .recorded { flex: 1; min-width: 12ch; color: var(--ga-text-secondary); font-size: 11px; }
      .bar { position: relative; flex: 0 0 150px; height: 18px; border: 1px solid var(--ga-border-subtle); border-radius: 2px; background: var(--ga-surface-inset); cursor: ew-resize; outline: none; }
      .bar:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .bar .fill { position: absolute; top: 0; bottom: 0; background: var(--ga-accent); opacity: 0.6; }
      .bar .centre { position: absolute; top: 0; bottom: 0; left: 50%; width: 1px; background: var(--ga-border-strong); }
      .bar .value { position: absolute; inset: 0; font-size: 10px; line-height: 16px; text-align: center; white-space: nowrap; pointer-events: none; font-variant-numeric: tabular-nums; }
      .bar[aria-disabled="true"] { cursor: not-allowed; opacity: 0.45; }
      ul { display: grid; gap: 2px; margin: 0; padding-left: 18px; }
      .notes { color: var(--ga-text-secondary); font-size: 11px; }
      .why { margin: 0; color: var(--ga-text-secondary); font-size: 11px; }
      .status { color: var(--ga-text-secondary); font-size: 12px; }
      .status[data-problem] { color: var(--ga-notice-warning); }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    if (store.topology(deviceId) === undefined) return;
    const channels = store.channels(deviceId);
    const effects = store.effects(deviceId);
    const asking = store.view<MidSideAsk | undefined>(`mid-side:${deviceId}:ask`, undefined);
    const said = signal<{ text: string; problem: boolean } | undefined>(undefined);
    const busy = signal(false);
    const rows = h("div", { class: "rows" });
    const box = h("div", { class: "confirm", role: "group", "aria-label": "Mid-side monitoring", "data-testid": "mid-side-confirm", hidden: true });
    const status = h("div", { class: "status", role: "status", "data-testid": "mid-side-status", hidden: true });
    this.root.replaceChildren(box, rows, status);

    const name = (id: string) => {
      const channel = channels.channel(id);
      return channel === undefined ? "a removed channel" : channels.displayName(channel);
    };

    /** Carries a confirmed plan out and says how it went. */
    const carryOut = (plan: Plan, done: string, after?: () => void) => {
      if (busy.peek()) return;
      busy.value = true;
      plan.apply().then(
        () => {
          said.value = { text: done, problem: false };
          after?.();
        },
        (error: unknown) => (said.value = { text: `Stopped: ${error instanceof Error ? error.message : String(error)}${error instanceof UndoneError ? "" : " The changes listed before that one were made."}`, problem: true }),
      ).finally(() => {
        asking.value = undefined;
        busy.value = false;
      });
    };

    // An effect's settings are known only once read: a decode's two effects have them read here, so
    // the guard can tell a polarity switch turned off, or settings apart, from ones it has not seen.
    const read = new Set<string>();
    this.watch(() => {
      for (const { decode } of decodes(store, deviceId)) {
        const type = decode.effect_type;
        if (retired(decode) || type === undefined || !store.connected.value) continue;
        for (const inst of [decode.left_effect_inst, decode.effect_inst]) {
          if (inst === undefined || read.has(`${type}:${inst}`)) continue;
          read.add(`${type}:${inst}`);
          untracked(() => void effects.loadCatalogue().then(() => effects.readParameters(type, inst)));
        }
      }
    });

    // The decodes' lines: rebuilt when what they say changes, never while a Width is only moving.
    let built = "";
    let widths: (() => void)[] = [];
    this.onDisconnect(() => widths.splice(0).forEach((dispose) => dispose()));
    this.watch(() => {
      const mix = channels.meteredMix.value;
      const mixName = channels.mixName(mix);
      const connected = store.connected.value;
      const open = asking.value;
      const found = decodes(store, deviceId).map((d) => ({
        id: d.group.id,
        title: d.group.name,
        plays: playsIn(store, deviceId, d, mix),
        retired: retired(d.decode),
        whole: d.mid !== undefined && d.left !== undefined && d.inverted !== undefined,
        problems: problems(store, deviceId, d.group.id, mix),
        recorded: `Recorded raw: ${channels.sourceLabel(d.decode.mid_source)} and ${channels.sourceLabel(d.decode.side_source)}` + (retired(d.decode) ? "." : ". The two side strips are effect chains' outputs and are not recorded."),
      }));
      const key = JSON.stringify([mix, mixName, connected, open, found.map((d) => [d.id, d.title, d.plays, d.retired, d.whole, d.recorded, d.problems.map((p) => [p.text, p.fix?.line])])]);
      if (key === built) return;
      built = key;
      untracked(() => {
        widths.splice(0).forEach((dispose) => dispose());
        rows.replaceChildren(
          ...found.map((d) => {
            const line = h("div", { class: "line" }, h("span", { class: "title" }, d.title));
            // A decode saved by an earlier way is not checked any more: it offers nothing but Remove.
            if (!d.retired && d.plays && d.whole) {
              const fill = h("div", { class: "fill" });
              const value = h("span", { class: "value" });
              const bar = h("div", { class: "bar", role: "slider", tabindex: 0, "aria-label": `${d.title} width`, "aria-valuemin": -WIDTH_RANGE, "aria-valuemax": WIDTH_RANGE, "data-testid": `mid-side-width-${d.id}`, "data-explain": "mid-side.width", "data-explain-name": d.title }, h("div", { class: "centre" }), fill, value);
              const current = () => width(store, deviceId, d.id, mix) ?? 0;
              // Wider is louder, so a finger raises it no faster than it raises a fader, and a
              // double-click resets to where it already is: there is no safe width to jump to.
              bindControl(bar, {
                axis: "x",
                min: -WIDTH_RANGE,
                max: WIDTH_RANGE,
                up: 1,
                page: 6,
                get reset() {
                  return current();
                },
                get: current,
                set: (v) => setWidth(store, deviceId, d.id, mix, Math.min(WIDTH_RANGE, Math.max(-WIDTH_RANGE, v))),
                enabled: () => store.connected.peek(),
                touchRise: TOUCH_RISE_PER_S,
              });
              bar.setAttribute("aria-disabled", String(!connected));
              bar.title = `Width in ${mixName}: the side strips' level against the mid's. It moves both side strips; the mid stays where it is.`;
              widths.push(
                effect(() => {
                  const now = current();
                  const shown = Math.min(WIDTH_RANGE, Math.max(-WIDTH_RANGE, now));
                  const position = ((shown + WIDTH_RANGE) / (2 * WIDTH_RANGE)) * 100;
                  fill.style.cssText = position >= 50 ? `left: 50%; width: ${position - 50}%` : `left: ${position}%; width: ${50 - position}%`;
                  value.textContent = `Width ${widthText(now)}`;
                  bar.setAttribute("aria-valuenow", String(now));
                  bar.setAttribute("aria-valuetext", widthText(now));
                }),
              );
              line.append(bar);
            } else if (!d.retired && d.whole) {
              const add = h("button", { type: "button", "data-testid": `mid-side-add-${d.id}`, "data-explain": "mid-side.add", title: `List what playing ${d.title} decoded in ${mixName} would change, then do it if you confirm`, "on:click": () => (asking.value = { kind: "add", group: d.id }) }, `Play in ${mixName} too...`);
              add.disabled = !connected || open !== undefined;
              line.append(add);
            }
            line.append(h("span", { class: "recorded", "data-testid": `mid-side-recorded-${d.id}` }, d.recorded));
            const remove = h("button", { type: "button", "data-testid": `mid-side-remove-${d.id}`, "data-explain": "mid-side.remove", title: "List what removing this decode puts back, then do it if you confirm", "on:click": () => (asking.value = { kind: "remove", group: d.id }) }, "Remove...");
            remove.disabled = !connected || open !== undefined;
            line.append(remove);
            const row = h("div", { class: "row", "data-testid": `mid-side-row-${d.id}` }, line);
            if (d.problems.length > 0) {
              row.setAttribute("data-warning", "");
              row.append(h("div", { class: "title" }, d.retired ? `${d.title} is no longer supported` : `${d.title} is not decoding properly${d.plays ? ` in ${mixName}` : ""}`), h("ul",{ "data-testid": `mid-side-problems-${d.id}` }, d.problems.map((p) => h("li", {}, p.text))));
              if (d.problems.some((p) => p.fix !== undefined)) {
                const repair = h("button", { type: "button", "data-testid": `mid-side-repair-${d.id}`, "data-explain": "mid-side.repair", title: "List the changes that would mend this, then make them if you confirm", "on:click": () => (asking.value = { kind: "repair", group: d.id }) }, "Put it back...");
                repair.disabled = !connected || open !== undefined;
                row.append(h("div", { class: "line" }, repair));
              }
            }
            return row;
          }),
        );
      });
    });

    // The confirm: what is asked, its plan as it stands now, and nothing sent until Confirm.
    this.watch(() => {
      const ask = asking.value;
      const mix = channels.meteredMix.value;
      const mixName = channels.mixName(mix);
      if (ask === undefined) {
        untracked(() => {
          box.hidden = true;
          box.replaceChildren();
        });
        return;
      }
      // While a confirmed plan is being carried out the list stays as it was confirmed: the plan
      // worked out again halfway through would be of a half-made decode.
      if (busy.value) {
        untracked(() => {
          for (const control of box.querySelectorAll<HTMLButtonElement | HTMLSelectElement>("button, select")) control.disabled = true;
        });
        return;
      }
      let heading: string;
      let planned: Planned;
      let done: string;
      let after: (() => void) | undefined;
      const options: HTMLElement[] = [];
      if (ask.kind === "setup") {
        const mid = channels.layout.value.channels.find((c) => c.id === ask.mid);
        const side = channels.layout.value.channels.find((c) => c.id === ask.side);
        heading = `Monitor ${name(ask.mid)} and ${name(ask.side)} as M/S in ${mixName}?`;
        done = `${name(ask.mid)} and ${name(ask.side)} are monitored as M/S in ${mixName}.`;
        after = () => store.softLink.clear();
        if (mid === undefined || side === undefined) {
          planned = { ok: false, why: "One of the two channels has been removed." };
        } else {
          planned = setupPlan(store, deviceId, mix, mid.id, side.id);
          const swap = h("button", { type: "button", "data-testid": "mid-side-swap", "data-explain": "mid-side.swap", title: "Make the other channel the mid", "on:click": () => (asking.value = { kind: "setup", mid: ask.side, side: ask.mid }) }, "Swap");
          options.push(h("div", { class: "line" }, h("span", { "data-testid": "mid-side-roles" }, `Mid: ${name(ask.mid)}. Side: ${name(ask.side)}.`), swap));
        }
      } else {
        const title = decodes(store, deviceId).find((d) => d.group.id === ask.group)?.group.name ?? "This decode";
        if (ask.kind === "add") {
          heading = `Play ${title} decoded in ${mixName} too?`;
          planned = addToMixPlan(store, deviceId, ask.group, mix);
          done = `${title} plays decoded in ${mixName} too.`;
        } else if (ask.kind === "repair") {
          heading = `Put ${title} back as it should be?`;
          planned = repairPlan(store, deviceId, ask.group, mix);
          done = `${title} is put back.`;
        } else {
          heading = `Remove ${title}?`;
          planned = removePlan(store, deviceId, ask.group);
          done = `${title} is removed: the pans, links and channels are as they were before it.`;
        }
      }
      const result = planned;
      untracked(() => {
        const cancel = h("button", { type: "button", "data-testid": "mid-side-cancel", "data-explain": "mid-side.cancel", "on:click": () => (asking.value = undefined) }, "Cancel");
        const children: (HTMLElement | string)[] = [h("div", { class: "title" }, heading), ...options];
        if (result.ok) {
          const go = h("button", { type: "button", "data-testid": "mid-side-go", "data-explain": "mid-side.confirm", "on:click": () => carryOut(result.plan, done, after) }, "Confirm");
          go.disabled = !store.connected.peek();
          children.push(
            h("div", {}, result.plan.lines.length === 0 ? "Nothing needs changing." : "These changes are made, and nothing before you confirm:"),
            h("ul", { "data-testid": "mid-side-plan" }, result.plan.lines.map((line) => h("li", {}, line))),
            h("ul", { class: "notes", "data-testid": "mid-side-notes" }, result.plan.notes.map((note) => h("li", {}, note))),
            h("div", { class: "line" }, go, cancel),
          );
        } else {
          children.push(h("p", { class: "why", role: "alert", "data-testid": "mid-side-refused" }, result.why), h("div", { class: "line" }, cancel));
        }
        box.replaceChildren(...children);
        box.hidden = false;
      });
    });

    this.watch(() => {
      const now = said.value;
      status.hidden = now === undefined;
      status.textContent = now?.text ?? "";
      status.toggleAttribute("data-problem", now?.problem === true);
    });
    // What the last change came to belongs to the mix it was made in, and goes once something else is asked.
    let shown = channels.meteredMix.peek();
    this.watch(() => {
      const mix = channels.meteredMix.value;
      const open = asking.value;
      if (mix === shown && open === undefined) return;
      const moved = mix !== shown;
      shown = mix;
      untracked(() => {
        said.value = undefined;
        if (moved && open !== undefined && !busy.peek()) asking.value = undefined;
      });
    });
    this.watch(() => {
      this.hidden = decodes(store, deviceId).length === 0 && asking.value === undefined && said.value === undefined;
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-mid-side": GaMidSide;
  }
}
