// <ga-channel device-id="…" channel-id="…">: one channel the user made (plan 2026-09-16). Its head
// holds the name, the input and main-mix menus, a send to each other mix (on/off and its level in
// that mix) and, when the input is a preamp, that preamp's controls, which every channel on the
// input and the Inputs page share. Below is the mixer strip for the main mix, inactive until the
// channel has both an input and a main mix. The preamp area keeps its space when unused, so faders
// line up across channels.

import { h } from "../core/dom.ts";
import { GAIN_RANGE, PREAMP_TYPES, type PreampType } from "../store/inputs.ts";
import { formatLevel, LEVEL_MAX } from "../store/mixer.ts";
import { bindControl, type ControlOptions } from "./controls.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";

/** How long a first click on 48V or remove waits for its confirmation. */
const ARM_MS = 3000;

const formatGain = (db: number) => `${db > 0 ? "+" : ""}${db} dB`;

export class GaChannel extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
      .head { display: grid; gap: 3px; min-width: 0; margin: 0; padding: 4px 3px; border: 0; border-radius: 3px; background: var(--ga-surface-raised); }
      .head select, .head input { width: 100%; min-width: 0; min-height: 20px; padding: 0 2px; font-size: 10px; }
      /* Captions keep their line when empty, so every channel head is the same height and faders line up. */
      .caption { min-height: 1.4em; font-size: 9px; color: var(--ga-text-muted); text-transform: uppercase; letter-spacing: 0.06em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .row { display: flex; gap: 2px; }
      .row button { flex: 1; min-width: 0; min-height: 18px; padding: 0; font-size: 10px; font-weight: 700; }
      .sends { display: grid; gap: 2px; }
      .send { display: grid; grid-template-columns: minmax(18px, 26px) 1fr; gap: 2px; }
      /* Every mix has a row, so heads match; the main mix's row says so instead of offering a send. */
      .send[data-main] .bar, .send:not([data-main]) .main-label { display: none; }
      .main-label { font-size: 9px; line-height: 16px; text-align: center; color: var(--ga-text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
      .send button { min-width: 0; min-height: 16px; padding: 0; font-size: 9px; font-weight: 700; }
      .send button[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .bar { position: relative; min-width: 0; height: 16px; border: 1px solid var(--ga-border-subtle); border-radius: 2px; background: var(--ga-surface-inset); cursor: ew-resize; touch-action: none; outline: none; }
      .bar:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .bar .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
      .bar .value { position: absolute; inset: 0; font-size: 9px; line-height: 14px; text-align: center; white-space: nowrap; overflow: hidden; pointer-events: none; font-variant-numeric: tabular-nums; }
      .bar[aria-disabled="true"] { cursor: not-allowed; opacity: 0.45; }
      .pre { display: grid; gap: 2px; padding-top: 3px; border-top: 1px solid var(--ga-border-subtle); }
      .pre[data-empty] { visibility: hidden; }
      .phantom[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
      .phantom[data-armed], .remove[data-armed] { outline: 2px dashed var(--ga-state-solo); outline-offset: -2px; }
      .phase[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .strip-slot { display: flex; flex: 1; min-height: 0; }
      .strip-slot ga-strip { flex: 1; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const id = this.getAttribute("channel-id") ?? "";
    const topology = store.topology(deviceId);
    const channels = store.channels(deviceId);
    const initial = channels.channel(id);
    if (topology === undefined || initial === undefined) {
      this.root.replaceChildren();
      return;
    }
    const slot = initial.slot;
    const label = `Channel ${slot + 1}`;
    const current = () => channels.channel(id);
    const usable = () => store.connected.peek();

    // Name, input, main mix.
    const name = h("input", { type: "text", "aria-label": `${label} name`, placeholder: `Ch ${slot + 1}`, "data-testid": `name-${slot}` });
    commitOnEnter(name, (value) => channels.rename(id, value.trim()), () => current()?.name ?? "");
    const input = h(
      "select",
      {
        "aria-label": `${label} input`,
        "data-testid": `in-${slot}`,
        "on:change": () => {
          const [group, channel] = input.value.split(":").map(Number);
          void channels.setSource(id, input.value === "" ? undefined : { group: group as number, channel: channel as number });
        },
      },
      h("option", { value: "" }, "No input"),
      topology.inputs.flatMap((group, g) =>
        group.type === "MUTE" ? [] : [h("optgroup", { label: group.name }, Array.from({ length: group.channels }, (_, c) => h("option", { value: `${g}:${c}` }, channels.sourceLabel({ group: g, channel: c }))))],
      ),
    );
    const output = h("select", {
      "aria-label": `${label} main mix`,
      "data-testid": `out-${slot}`,
      "on:change": () => void channels.setMainMix(id, output.value === "" ? undefined : Number(output.value)),
    });

    // Sends: one row per mix; the main mix's row is marked "Main" and offers no send.
    const sendRows = Array.from({ length: channels.mixCount }, (_, mix) => {
      const mixer = store.mixer(deviceId, mix);
      const toggle = h("button", { type: "button", "data-testid": `send-${slot}-${mix}`, "on:click": () => void channels.setSend(id, mix, !(current()?.sends.includes(mix) ?? false)) }, `M${mix + 1}`);
      const fill = h("div", { class: "fill" });
      const value = h("span", { class: "value" });
      const level = h("div", { class: "bar", role: "slider", tabindex: 0, "aria-label": `${label} send level to mix ${mix + 1}`, "aria-valuemin": -LEVEL_MAX, "aria-valuemax": 0, "data-testid": `send-level-${slot}-${mix}` }, fill, value);
      const sending = () => {
        const ch = current();
        return ch !== undefined && channels.isActive(ch) && ch.sends.includes(mix);
      };
      // Level is attenuation, so the bar runs from −90 dB at the left to 0 dB at the right.
      bindControl(level, { axis: "x", min: LEVEL_MAX, max: 0, up: -1, page: 6, reset: 0, get: () => mixer.strip(slot).peek().level, set: (v) => mixer.setLevel(slot, v), enabled: () => usable() && sending() });
      const row = h("div", { class: "send" }, toggle, level, h("span", { class: "main-label" }, "Main"));
      this.watch(() => {
        const ch = channels.layout.value.channels.find((c) => c.id === id);
        if (ch === undefined) return;
        const on = ch.sends.includes(mix);
        const main = ch.main_mix === mix;
        row.toggleAttribute("data-main", main);
        toggle.disabled = main;
        toggle.setAttribute("aria-pressed", String(on));
        toggle.title = main ? `${channels.mixName(mix)} is this channel's main mix` : `Send to ${channels.mixName(mix)}`;
        const attenuation = mixer.strip(slot).value.level;
        fill.style.width = `${((LEVEL_MAX - attenuation) / LEVEL_MAX) * 100}%`;
        value.textContent = formatLevel(attenuation);
        level.setAttribute("aria-valuenow", String(-attenuation));
        level.setAttribute("aria-disabled", String(!(on && channels.isActive(ch) && store.connected.value)));
      });
      return row;
    });

    // The preamp this channel's input is on, if any.
    const inputs = store.inputs(deviceId);
    this.onDisconnect(inputs.activate());
    const preamp = () => {
      const source = current()?.source;
      return source !== undefined && topology.inputs[source.group]?.type === "PREAMP" ? source.channel : undefined;
    };
    const preTitle = h("span", { class: "caption" });
    const preType = h("select", {
      "aria-label": `${label} preamp type`,
      "data-testid": `pre-type-ch-${slot}`,
      "on:change": () => {
        const index = preamp();
        if (index !== undefined) inputs.setType(index, Number(preType.value) as PreampType);
      },
    });
    const gainFill = h("div", { class: "fill" });
    const gainValue = h("span", { class: "value" });
    const gain = h("div", { class: "bar", role: "slider", tabindex: 0, "aria-label": `${label} preamp gain`, "data-testid": `pre-gain-ch-${slot}` }, gainFill, gainValue);
    const gainRange: ControlOptions = {
      axis: "x",
      min: 0,
      max: 65,
      up: 1,
      page: 6,
      reset: 0,
      get: () => {
        const index = preamp();
        return index === undefined ? 0 : inputs.preamp(index).peek().gain;
      },
      set: (v) => {
        const index = preamp();
        if (index !== undefined) inputs.setGain(index, v);
      },
      enabled: () => usable() && preamp() !== undefined,
    };
    bindControl(gain, gainRange);
    let armTimer: ReturnType<typeof setTimeout> | undefined;
    const phantom = h(
      "button",
      {
        type: "button",
        class: "phantom",
        "data-testid": `pre-48v-ch-${slot}`,
        title: "48V: click twice, or Ctrl/Cmd+click, to turn on",
        "on:click": (event) => {
          const index = preamp();
          if (index === undefined) return;
          if (inputs.preamp(index).peek().phantom) inputs.setPhantom(index, false);
          else if (armTimer !== undefined || (event as MouseEvent).ctrlKey || (event as MouseEvent).metaKey) {
            disarm();
            inputs.setPhantom(index, true);
          } else {
            phantom.setAttribute("data-armed", "");
            phantom.textContent = "Sure?";
            armTimer = setTimeout(disarm, ARM_MS);
          }
        },
      },
      "48V",
    );
    const disarm = () => {
      clearTimeout(armTimer);
      armTimer = undefined;
      phantom.removeAttribute("data-armed");
      phantom.textContent = "48V";
    };
    this.onDisconnect(disarm);
    const phase = h(
      "button",
      {
        type: "button",
        class: "phase",
        "data-testid": `pre-phase-ch-${slot}`,
        "on:click": () => {
          const index = preamp();
          if (index !== undefined) inputs.setPhaseInvert(index, !inputs.preamp(index).peek().phaseInvert);
        },
      },
      "Ø",
    );
    const pre = h("div", { class: "pre", "data-testid": `pre-ch-${slot}` }, preTitle, preType, gain, h("div", { class: "row" }, phantom, phase));

    // Move and remove; remove asks for a second click, since it mutes the channel's routes.
    const order = () => channels.layout.peek().channels.findIndex((c) => c.id === id);
    const left = h("button", { type: "button", "aria-label": `Move ${label} left`, "data-testid": `move-left-${slot}`, "on:click": () => channels.move(id, order() - 1) }, "‹");
    const right = h("button", { type: "button", "aria-label": `Move ${label} right`, "data-testid": `move-right-${slot}`, "on:click": () => channels.move(id, order() + 1) }, "›");
    let removeTimer: ReturnType<typeof setTimeout> | undefined;
    const remove = h(
      "button",
      {
        type: "button",
        class: "remove",
        "aria-label": `Remove ${label}`,
        "data-testid": `remove-${slot}`,
        "on:click": () => {
          if (removeTimer === undefined) {
            remove.setAttribute("data-armed", "");
            removeTimer = setTimeout(() => {
              removeTimer = undefined;
              remove.removeAttribute("data-armed");
            }, ARM_MS);
            return;
          }
          clearTimeout(removeTimer);
          void channels.remove(id);
        },
      },
      "×",
    );
    this.onDisconnect(() => clearTimeout(removeTimer));

    const head = h("fieldset", { class: "head", "aria-label": label }, h("div", { class: "row" }, left, right, remove), name, input, output, h("span", { class: "caption" }, "Sends"), h("div", { class: "sends" }, sendRows), pre);
    const stripSlot = h("div", { class: "strip-slot" });
    this.root.replaceChildren(head, stripSlot);

    this.watch(() => {
      const ch = channels.layout.value.channels.find((c) => c.id === id);
      if (ch === undefined) return;
      if (this.root.activeElement !== name) name.value = ch.name;
      input.value = ch.source === undefined ? "" : `${ch.source.group}:${ch.source.channel}`;
      output.replaceChildren(h("option", { value: "" }, "No mix"), ...Array.from({ length: channels.mixCount }, (_, mix) => h("option", { value: String(mix) }, channels.mixName(mix))));
      output.value = ch.main_mix === undefined ? "" : String(ch.main_mix);
      const index = channels.layout.value.channels.findIndex((c) => c.id === id);
      left.disabled = index <= 0;
      right.disabled = index >= channels.layout.value.channels.length - 1;
      this.toggleAttribute("inactive", !channels.isActive(ch));
    });

    let preamps = -1;
    this.watch(() => {
      const ch = channels.layout.value.channels.find((c) => c.id === id);
      const source = ch?.source;
      const index = source !== undefined && topology.inputs[source.group]?.type === "PREAMP" ? source.channel : undefined;
      pre.toggleAttribute("data-empty", index === undefined);
      if (index === undefined) return;
      if (index !== preamps) {
        preamps = index;
        preType.replaceChildren(...PREAMP_TYPES.filter((t) => t.value !== 2 || index < inputs.hizCount).map((t) => h("option", { value: String(t.value) }, t.label)));
      }
      const s = inputs.preamp(index).value;
      const { min, max } = GAIN_RANGE[s.type as PreampType] ?? GAIN_RANGE[0];
      gainRange.min = min;
      gainRange.max = max;
      preTitle.textContent = `Preamp ${index + 1}`;
      preType.value = String(s.type);
      gainFill.style.width = `${((Math.min(max, Math.max(min, s.gain)) - min) / (max - min)) * 100}%`;
      gainValue.textContent = formatGain(s.gain);
      gain.setAttribute("aria-valuetext", formatGain(s.gain));
      phantom.setAttribute("aria-pressed", String(s.phantom));
      phantom.disabled = s.type !== 0;
      if (s.phantom) disarm();
      phase.setAttribute("aria-pressed", String(s.phaseInvert));
    });

    this.watch(() => {
      head.disabled = !store.connected.value;
    });

    // The strip is rebuilt when what it was rendered with changes (strips read attributes once).
    let rendered = "";
    this.watch(() => {
      const ch = channels.layout.value.channels.find((c) => c.id === id);
      if (ch === undefined) return;
      const active = channels.isActive(ch);
      const metered = active && ch.main_mix === channels.meteredMix.value;
      const stripLabel = ch.name !== "" ? ch.name : ch.source !== undefined ? channels.sourceLabel(ch.source) : "";
      const key = `${ch.main_mix ?? 0}|${stripLabel}|${active}|${metered}`;
      if (key === rendered) return;
      rendered = key;
      stripSlot.replaceChildren(h("ga-strip", { "device-id": deviceId, mixer: String(ch.main_mix ?? 0), strip: String(ch.slot), label: stripLabel, inactive: active ? undefined : "true", meter: metered ? undefined : "off" }));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-channel": GaChannel;
  }
}
