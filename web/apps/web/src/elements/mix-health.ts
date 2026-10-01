// What the selected mix plays that its channels do not show, on the Mixer page, fetched with that
// page's first visit (lazy.ts) so the app's entry chunk does not carry it:
//
// - <ga-mix-notice device-id="…">: a notice above the channels listing each stray, solo and audible
//   effect return by slot and source in plain words, with "Tidy this mix" behind a confirm that
//   lists every change (store/mix-tidy.ts), and says first when a change would break the phase path
//   of a dedicated cable, in the Routing page's words. After tidying it reads the mix again and says
//   how many changes were made, and what is left if anything is.
// - <ga-effect-returns device-id="…">: the Quadro's effect returns, AFX OUT 1 to 6 on slots 1 to 6
//   of every mix, as slim strips before the channels (level, mute and solo; the same `set_mixer` as
//   any strip). They show while one of them is audible or soloed, and a "Show effect returns" rail
//   shows or hides them by hand, which this browser remembers per device.

import { h } from "../core/dom.ts";
import { computed, signal, untracked } from "../core/signal.ts";
import { applyTidy, planLines, planSize, slotLabel, tidyPlan, type Naming } from "../store/mix-tidy.ts";
import { GaElement, phaseGuard, sheet, useStore } from "./element.ts";

/** A strip level in dB, as its readout shows it (the mixer's `formatLevel`, kept here so this chunk takes nothing of the mixer's). */
const formatLevel = (level: number) => `${level === 0 ? 0 : -level} dB`;

export class GaMixNotice extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; }
      :host([hidden]) { display: none; }
      .notice { display: grid; gap: 6px; padding: 8px 10px; border: 1px solid var(--ga-notice-warning); border-left-width: 4px; border-radius: 3px; background: var(--ga-surface-raised); font-size: 12px; line-height: 1.4; }
      .notice.quiet { border-color: var(--ga-border-strong); }
      .title { font-weight: 600; }
      ul { display: grid; gap: 2px; margin: 0; padding-left: 18px; }
      .actions { --ga-field-height: 26px; display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .confirm { display: grid; gap: 6px; padding: 8px; border-radius: 3px; background: var(--ga-surface-inset); }
      .confirm label { display: flex; align-items: center; gap: 6px; }
      .done { color: var(--ga-text-secondary); }
      .phase { display: grid; gap: 4px; padding: 6px 8px; border: 1px solid var(--ga-state-solo); border-radius: 3px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const topology = store.topology(deviceId);
    if (topology === undefined) return;
    const channels = store.channels(deviceId);
    const naming: Naming = { source: (source) => channels.sourceLabel(source), mute: store.routing(deviceId).mute };
    const box = h("div", { class: "notice", "data-testid": "mix-notice", "data-explain": "mixer.notice" });
    this.root.replaceChildren(box);
    // The confirm, open for one mix; whether it mutes the returns too; and the last tidy's outcome.
    const asking = signal<number | undefined>(undefined);
    const done = signal<{ mix: number; text: string } | undefined>(undefined);
    let muteReturns = false;
    let busy = false;

    this.watch(() => {
      const mix = channels.meteredMix.value;
      const health = store.mixHealth(deviceId, mix).value;
      const outcome = done.value?.mix === mix ? done.value.text : undefined;
      const open = asking.value === mix && health !== undefined;
      const name = channels.mixName(mix);
      untracked(() => {
        const items = health === undefined ? [] : [...health.strays, ...health.solos];
        const returns = health?.returns ?? [];
        this.hidden = items.length === 0 && returns.length === 0 && outcome === undefined;
        box.classList.toggle("quiet", items.length === 0);
        if (this.hidden || health === undefined) {
          box.replaceChildren();
          return;
        }
        const lines = [
          ...health.strays.map((s) =>
            s.channel === undefined
              ? `Slot ${s.slot + 1}: ${slotLabel(naming, s)} plays at ${formatLevel(s.level)} but is not one of this mix's channels.`
              : `Slot ${s.slot + 1}: ${channels.displayName(s.channel)} should take ${s.channel.source === undefined ? "its input" : channels.sourceLabel(s.channel.source)}, but the device plays ${slotLabel(naming, s)} there at ${formatLevel(s.level)}.`,
          ),
          ...health.solos.map(
            (s) =>
              `${slotLabel(naming, s)} (slot ${s.slot + 1}) is soloed, which silences every other channel in this mix.` +
              (s.shown ? "" : s.slot < channels.firstSlot ? " It is an effect return strip." : " It is not one of this mix's channels, so no strip here shows it."),
          ),
        ];
        const children: (HTMLElement | string)[] = [];
        if (items.length > 0) children.push(h("div", { class: "title" }, `${name} plays more than its channels show`), h("ul", { "data-testid": "mix-notice-items" }, lines.map((line) => h("li", {}, line))));
        if (returns.length > 0) {
          children.push(
            h(
              "div",
              { "data-testid": "mix-notice-returns" },
              `Effect return${returns.length === 1 ? "" : "s"} playing: ${returns.map((s) => `${slotLabel(naming, s)} at ${formatLevel(s.level)}`).join(", ")}. They may be deliberate; the effect return strips before the channels show and set them.`,
            ),
          );
        }
        if (outcome !== undefined) children.push(h("div", { class: "done", "data-testid": "mix-tidied" }, outcome));
        if (items.length > 0 && !open) {
          const tidy = h("button", { type: "button", "data-testid": "mix-tidy", "data-explain": "mixer.tidy", title: "List the changes that would undo this, then make them if you confirm" }, "Tidy this mix");
          tidy.addEventListener("click", () => {
            muteReturns = false;
            asking.value = mix;
          });
          tidy.disabled = !store.connected.peek();
          children.push(h("div", { class: "actions" }, tidy));
        }
        if (open) {
          const list = h("ul", { "data-testid": "mix-tidy-plan" });
          const fill = () => list.replaceChildren(...planLines(tidyPlan(health, muteReturns), naming, (id) => {
            const channel = channels.channel(id);
            return channel === undefined ? "The channel" : channels.displayName(channel);
          }).map((line) => h("li", {}, line)));
          fill();
          const extra: HTMLElement[] = [];
          // A layout input put back can be the playback channel a dedicated cable keeps.
          const kept =
            phaseGuard()?.routes(deviceId, store.mixInput(deviceId, mix), tidyPlan(health, false).routes.map((r) => ({ channel: r.slot, source: r.to === null ? null : { source: r.to.group, channel: r.to.channel } }))) ?? [];
          if (kept.length > 0) extra.push(h("div", { class: "phase", "data-testid": "mix-tidy-phase" }, "This breaks the phase path of a dedicated cable:", h("ul", {}, kept.map((line) => h("li", {}, line)))));
          if (returns.length > 0) {
            const tick = h("input", { type: "checkbox", "data-testid": "mix-tidy-returns", "data-explain": "mixer.tidy-returns" });
            tick.checked = muteReturns;
            tick.addEventListener("change", () => {
              muteReturns = tick.checked;
              fill();
            });
            extra.push(h("label", {}, tick, "Also mute the effect returns"));
          }
          const go = h("button", { type: "button", "data-testid": "mix-tidy-confirm", "data-explain": "mixer.tidy-confirm" }, "Tidy");
          const cancel = h("button", { type: "button", "data-testid": "mix-tidy-cancel", "data-explain": "mixer.tidy-cancel", "on:click": () => (asking.value = undefined) }, "Cancel");
          go.addEventListener("click", () => {
            if (busy) return;
            busy = true;
            go.disabled = true;
            const plan = tidyPlan(health, muteReturns);
            void applyTidy(store, deviceId, mix, plan)
              .then((changes) => {
                const left = store.mixWarning(deviceId, mix);
                const size = planSize(plan);
                done.value = {
                  mix,
                  text: `Tidied: ${changes} ${changes === 1 ? "change" : "changes"}.${changes < size ? ` ${size - changes} could not be sent.` : ""}${left === undefined ? "" : ` Read back from the device, ${left}.`}`,
                };
              })
              .finally(() => {
                busy = false;
                asking.value = undefined;
              });
          });
          children.push(h("div", { class: "confirm", "data-testid": "mix-tidy-confirm-box" }, h("div", { class: "title" }, `Tidy ${name}? These changes are sent to the device:`), list, ...extra, h("div", { class: "actions" }, go, cancel)));
        }
        box.replaceChildren(...children);
      });
    });
    // A tidy's outcome belongs to its mix: choosing another forgets it.
    let shown = channels.meteredMix.peek();
    this.watch(() => {
      const mix = channels.meteredMix.value;
      if (mix !== shown) {
        shown = mix;
        untracked(() => {
          done.value = undefined;
          asking.value = undefined;
        });
      }
    });
  }
}

/** Where this browser keeps each device's choice to show or hide the effect returns. */
export const EFFECT_RETURNS_STORAGE_KEY = "gazelle.mixer.effectReturns";

function storedChoices(): Record<string, boolean> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(EFFECT_RETURNS_STORAGE_KEY) ?? "{}");
    return typeof parsed === "object" && parsed !== null ? (parsed as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

export class GaEffectReturns extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex: 0 0 auto; gap: 2px; margin-right: 6px; padding-right: 6px; border-right: 1px solid var(--ga-border-subtle); }
      .rail { min-height: 0; padding: 8px 3px; writing-mode: vertical-rl; transform: rotate(180deg); color: var(--ga-text-secondary); font-size: 10px; font-weight: 600; white-space: nowrap; }
      .rail[aria-pressed="true"] { border-color: var(--ga-accent); color: var(--ga-text-primary); }
      ga-strip { flex: 0 0 56px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const topology = store.topology(deviceId);
    if (topology?.family !== "quadro") return;
    const channels = store.channels(deviceId);
    const routing = store.routing(deviceId);
    const afx = topology.inputs.findIndex((g) => g.type === "AFX_OUT");
    const fixed = channels.firstSlot;
    const choice = signal<boolean | undefined>(storedChoices()[deviceId]);
    const audible = computed(() => {
      const health = store.mixHealth(deviceId, channels.meteredMix.value).value;
      return health !== undefined && (health.returns.length > 0 || health.solos.some((s) => s.slot < fixed));
    });
    const shown = computed(() => choice.value ?? audible.value);
    const rail = h("button", { type: "button", class: "rail", "data-testid": "effect-returns-toggle", "data-explain": "mixer.effect-returns" }, "Effect returns");
    rail.addEventListener("click", () => {
      const next = !shown.peek();
      choice.value = next;
      try {
        localStorage.setItem(EFFECT_RETURNS_STORAGE_KEY, JSON.stringify({ ...storedChoices(), [deviceId]: next }));
      } catch {
        // Not kept, as in a private window: it still holds for this page.
      }
    });
    let rendered = "";
    this.watch(() => {
      const on = shown.value;
      const mix = channels.meteredMix.value;
      rail.setAttribute("aria-pressed", String(on));
      rail.title = on ? "Hide effect returns" : "Show effect returns: AFX OUT 1 to 6 on slots 1 to 6 of this mix";
      const slots = routing.destination(store.mixInput(deviceId, mix)).value;
      // The slot's input as the device routes it; AFX OUT k where it is unread, as the vendor keeps it.
      const strips = on
        ? Array.from({ length: fixed }, (_, slot) => {
            const routed = slots?.[slot] ?? { source: afx, channel: slot };
            const muted = routed.source === routing.mute;
            return { slot, label: muted ? `Slot ${slot + 1}` : channels.sourceLabel({ group: routed.source, channel: routed.channel }), group: muted ? undefined : routed.source, channel: routed.channel, color: muted ? undefined : topology.inputs[routed.source]?.color };
          })
        : [];
      const key = JSON.stringify([mix, strips]);
      if (key === rendered) return;
      rendered = key;
      untracked(() =>
        this.root.replaceChildren(
          rail,
          ...strips.map((s) =>
            h("ga-strip", {
              "device-id": deviceId,
              mixer: String(mix),
              strip: String(s.slot),
              label: s.label,
              compact: true,
              fixed: true,
              "input-group": s.group,
              "input-channel": s.group === undefined ? undefined : s.channel,
              color: s.color,
              "data-testid": `effect-return-${s.slot}`,
            }),
          ),
        ),
      );
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-mix-notice": GaMixNotice;
    "ga-effect-returns": GaEffectReturns;
  }
}
