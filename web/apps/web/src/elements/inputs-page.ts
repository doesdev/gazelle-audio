// <ga-inputs device-id="…">: every hardware input on a device, shown whether or not a mixer channel
// uses it. Preamps have type, gain, 48V, phase invert and the HPF the device reports; digital inputs
// have gain, editable where the device's own panel sets it. 48V turns on only with a confirming
// second click or Ctrl/Cmd+click, as the Quadro panel guards it; turning it off is one click.

import { h } from "../core/dom.ts";
import { DIGITAL_GAIN, GAIN_RANGE, PREAMP_TYPES, type DigitalGroup, type InputsModel, type PreampType } from "../store/inputs.ts";
import { bindControl, type ControlOptions } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { LINK_STYLES, linkBar, linkButton } from "./link-bar.ts";
import { href } from "./router.ts";

/** How long a first 48V click waits for its confirmation. */
const ARM_MS = 3000;

const formatGain = (db: number) => `${db > 0 ? "+" : ""}${db} dB`;

export class GaInputs extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 12px; }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .spacer { flex: 1; }
      .note { margin: 0; font-size: 11px; color: var(--ga-text-muted); }
      /* One line, cut with an ellipsis; the full bytes are in the tooltip. */
      .last-sent { display: flex; min-width: 0; max-width: 100%; font-size: 11px; white-space: nowrap; }
      .last-sent code { min-width: 0; overflow: hidden; text-overflow: ellipsis; font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      h2 { margin: 0 0 6px; }
      /* One column grid for every section: a digital input takes one column, a preamp two, so edges line up across sections. */
      .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(72px, 1fr)); gap: 6px; }
      .preamp {
        display: grid;
        grid-column: span 2;
        gap: 6px;
        padding: 6px;
        border-radius: 3px;
        background: var(--ga-surface-raised);
      }
      .head { display: flex; align-items: baseline; justify-content: space-between; }
      .name { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; }
      .hpf { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-muted); background: var(--ga-surface-inset); }
      .hpf[data-on] { color: var(--ga-text-inverse); background: var(--ga-accent); }
      .badges { display: flex; align-items: center; gap: 4px; }
      .link { min-width: 0; min-height: 16px; padding: 0 5px; font-size: 11px; line-height: 1; }
      ${LINK_STYLES}
      .segmented { display: flex; }
      .segmented button { flex: 1; min-width: 0; min-height: 22px; padding: 0 4px; border-radius: 0; font-size: 11px; font-weight: 600; }
      .segmented button + button { margin-left: -1px; }
      .segmented button:first-child { border-radius: 3px 0 0 3px; }
      .segmented button:last-child { border-radius: 0 3px 3px 0; }
      .segmented button[aria-pressed="true"] { position: relative; border-color: var(--ga-accent); background: var(--ga-accent); color: var(--ga-accent-text); }
      .gain {
        position: relative;
        height: 22px;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 3px;
        background: var(--ga-surface-inset);
        cursor: ew-resize;
        touch-action: none;
        outline: none;
      }
      .gain:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .gain .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
      .gain .value { position: absolute; inset: 0; font-size: 11px; line-height: 20px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
      .gain[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .mics { display: grid; gap: 6px; max-width: 640px; }
      .mic { display: grid; grid-template-columns: minmax(64px, 88px) minmax(0, 1fr) minmax(0, 1.4fr) auto; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 3px; background: var(--ga-surface-raised); }
      .mic select { min-width: 0; min-height: 24px; }
      .mic .name { font-size: 12px; color: var(--ga-text-secondary); }
      .swap { font-size: 11px; font-weight: 700; }
      .swap[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .note-inline { margin: 6px 0 0; font-size: 11px; color: var(--ga-text-muted); }
      @media (max-width: 560px) {
        .mic { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto; }
        .mic .name { grid-column: 1 / -1; }
      }
      .toggles { display: flex; gap: 4px; }
      .toggles button { flex: 1; min-width: 0; font-size: 11px; font-weight: 700; }
      .phantom[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
      .phantom[data-armed] { outline: 2px dashed var(--ga-state-solo); outline-offset: -2px; }
      .phase[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .gain.readonly { cursor: default; }
      .cell { display: grid; gap: 2px; padding: 4px; border-radius: 3px; background: var(--ga-surface-raised); }
      .cell .label { font-size: 10px; color: var(--ga-text-secondary); }
      .cell-head { display: flex; align-items: center; justify-content: space-between; gap: 4px; min-height: 16px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    if (store.topology(deviceId) === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so its inputs are not known.`));
      return;
    }
    const inputs = store.inputs(deviceId);
    this.onDisconnect(inputs.activate());
    // Pairs linked on the device (by its own panel) become links, unless already in one.
    void store.links.importDevicePairs(deviceId);
    const enabled = () => store.connected.peek();

    const devices = h("select", {
      "aria-label": "Device",
      "on:change": (event) => {
        location.hash = href({ page: "inputs", id: (event.target as HTMLSelectElement).value });
      },
    });
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent" });
    const note = h("p", { class: "note" });

    const preamps = h("div", { class: "grid" }, Array.from({ length: inputs.preampCount }, (_, i) => this.#preamp(inputs, i, enabled)));
    const sections = inputs.digital.filter((group) => group.count > 0).map((group) => h("section", {}, h("h2", {}, group.label), h("div", { class: "grid" }, Array.from({ length: group.count }, (_, i) => this.#digital(inputs, group, i, enabled)))));

    // Mic emulation, Quadro only: which Antelope microphone is on a preamp and what it is made to
    // sound like. It has its own section because two catalogues do not fit in a preamp's column,
    // and because it only applies to a preamp on Mic.
    const emulation = !inputs.hasMicEmulation
      ? undefined
      : h(
          "section",
          {},
          h("h2", {}, "Mic emulation"),
          h("div", { class: "mics" }, Array.from({ length: inputs.preampCount }, (_, i) => this.#emulation(inputs, i))),
          h("p", { class: "note-inline" }, "For Antelope's own microphones on a preamp set to Mic. The stereo pattern presets of the dual-capsule microphones are not set here."),
        );
    void inputs.loadEmulations();

    const links = linkBar((fn) => this.watch(fn), deviceId);
    this.root.replaceChildren(
      h("div", { class: "bar" }, devices, h("span", { class: "spacer" }), lastSent),
      links,
      note,
      h("section", {}, h("h2", {}, "Preamps"), preamps),
      ...sections,
      ...(emulation === undefined ? [] : [emulation]),
    );

    this.watch(() => {
      const known = store.devices.value.filter((d) => d.family !== null);
      devices.replaceChildren(...known.map((d) => h("option", { value: d.id, selected: d.id === deviceId }, d.model ?? d.id)));
      devices.value = deviceId;
      devices.disabled = !store.connected.value;
    });
    this.watch(() => {
      note.textContent = inputs.preampCount > 0 && !inputs.preamp(0).value.known ? "The device has not reported its inputs yet, so controls start at defaults and send when changed." : "";
    });
    this.watch(() => {
      const sent = store.lastSent.value;
      const dryRun = store.server.value.dry_run;
      if (sent === undefined || sent.deviceId !== deviceId) {
        lastSent.textContent = dryRun ? "Dry run: nothing is written to the device" : "";
        return;
      }
      lastSent.replaceChildren(`${sent.dryRun ? "Dry run, would send" : "Sent"} ${sent.command}: `, h("code", { title: sent.hex }, sent.hex));
    });
    this.watch(() => {
      const connected = store.connected.value;
      for (const button of this.root.querySelectorAll<HTMLButtonElement>("button[data-control]")) button.disabled = !connected || button.hasAttribute("data-unavailable");
      for (const control of this.root.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!connected));
    });
  }

  #preamp(inputs: InputsModel, i: number, enabled: () => boolean): HTMLElement {
    const label = `Preamp ${i + 1}`;
    const state = inputs.preamp(i);
    const types = PREAMP_TYPES.filter((t) => t.value !== 2 || i < inputs.hizCount).map((t) =>
      h(
        "button",
        {
          type: "button",
          "data-control": "",
          "data-testid": `pre-type-${i}-${t.label.toLowerCase()}`,
          "aria-label": `${label} ${t.label}`,
          "on:click": () => {
            try {
              inputs.setType(i, t.value);
            } catch (error) {
              useStore().reportError(error instanceof Error ? error.message : String(error));
            }
          },
        },
        t.label,
      ),
    );

    const fill = h("div", { class: "fill" });
    const value = h("span", { class: "value" });
    const gain = h("div", { class: "gain", role: "slider", tabindex: 0, "aria-label": `${label} gain`, "data-testid": `pre-gain-${i}` }, fill, value);
    const range: ControlOptions = { axis: "x", min: 0, max: 65, up: 1, page: 6, reset: 0, get: () => state.peek().gain, set: (v) => inputs.setGain(i, v), enabled };
    bindControl(gain, range);

    let armTimer: ReturnType<typeof setTimeout> | undefined;
    const disarm = () => {
      clearTimeout(armTimer);
      armTimer = undefined;
      phantom.removeAttribute("data-armed");
      phantom.textContent = "48V";
    };
    const phantom = h(
      "button",
      {
        type: "button",
        class: "phantom",
        "data-control": "",
        "data-testid": `pre-48v-${i}`,
        "aria-label": `${label} 48V phantom power`,
        title: "48V: click twice, or Ctrl/Cmd+click, to turn on",
        "on:click": (event) => {
          if (state.peek().phantom) {
            inputs.setPhantom(i, false);
          } else if (armTimer !== undefined || (event as MouseEvent).ctrlKey || (event as MouseEvent).metaKey) {
            disarm();
            inputs.setPhantom(i, true);
          } else {
            // A link turns 48V on at every member on Mic, so the confirmation says how many.
            const store = useStore();
            const link = store.links.linkOf("preamp", inputs.deviceId, i);
            const count = link === undefined ? 1 : link.members.filter((m) => store.devices.peek().some((d) => d.id === m.device_id && d.family !== null) && store.inputs(m.device_id).preamp(m.channel).peek().type === 0).length;
            phantom.setAttribute("data-armed", "");
            phantom.textContent = count > 1 ? `Confirm ${count}` : "Confirm";
            armTimer = setTimeout(disarm, ARM_MS);
          }
        },
      },
      "48V",
    );
    this.onDisconnect(disarm);
    const phase = h("button", { type: "button", class: "phase", "data-control": "", "data-testid": `pre-phase-${i}`, "aria-label": `${label} phase invert`, "on:click": () => inputs.setPhaseInvert(i, !state.peek().phaseInvert) }, "Ø");
    const hpf = h("span", { class: "hpf", "data-testid": `pre-hpf-${i}`, title: "High-pass filter, as the device reports it" }, "HPF");
    const link = linkButton((fn) => this.watch(fn), "preamp", inputs.deviceId, i, `pre-link-${i}`);

    this.watch(() => {
      const s = state.value;
      const { min, max } = GAIN_RANGE[s.type as PreampType] ?? GAIN_RANGE[0];
      range.min = min;
      range.max = max;
      for (const [n, button] of types.entries()) button.setAttribute("aria-pressed", String(PREAMP_TYPES[n]?.value === s.type));
      fill.style.width = `${((Math.min(max, Math.max(min, s.gain)) - min) / (max - min)) * 100}%`;
      value.textContent = formatGain(s.gain);
      gain.setAttribute("aria-valuemin", String(min));
      gain.setAttribute("aria-valuemax", String(max));
      gain.setAttribute("aria-valuenow", String(s.gain));
      gain.setAttribute("aria-valuetext", formatGain(s.gain));
      phantom.setAttribute("aria-pressed", String(s.phantom));
      // 48V exists for Mic only; the button stays visible so the state is never hidden.
      phantom.toggleAttribute("data-unavailable", s.type !== 0);
      phantom.disabled = s.type !== 0 || !enabled();
      if (s.phantom) disarm();
      phase.setAttribute("aria-pressed", String(s.phaseInvert));
      hpf.toggleAttribute("data-on", s.hpf);
    });

    return h("div", { class: "preamp", "data-testid": `preamp-${i}` }, h("div", { class: "head" }, h("span", { class: "name" }, label), h("span", { class: "badges" }, link, hpf)), h("div", { class: "segmented", role: "group", "aria-label": `${label} type` }, types), gain, h("div", { class: "toggles" }, phantom, phase));
  }

  /** One preamp's microphone and emulation. The catalogue is the microphone's, so it follows it. */
  #emulation(inputs: InputsModel, i: number): HTMLElement {
    const label = `Preamp ${i + 1}`;
    const state = inputs.emulation(i);
    const target = h(
      "select",
      { "aria-label": `${label} microphone`, "data-testid": `mic-target-${i}`, "on:change": () => inputs.setEmulationTarget(i, Number(target.value)) },
      inputs.micTargets.map((t) => h("option", { value: String(t.value) }, t.name)),
    );
    const model = h("select", { "aria-label": `${label} emulation`, "data-testid": `mic-model-${i}`, "on:change": () => inputs.setEmulationModel(i, Number(model.value)) });
    const swap = h("button", {
      type: "button",
      class: "swap",
      "data-control": "",
      "data-testid": `mic-swap-${i}`,
      "aria-label": `${label} swap the microphone's two sides`,
      title: "Swap the two sides of a dual-capsule microphone",
      "on:click": () => inputs.setEmulationSwap(i, !state.peek().swap),
    }, "Swap");

    this.watch(() => {
      const current = state.value;
      const models = inputs.emulationModels(current.target);
      if (this.root.activeElement !== target) target.value = String(current.target);
      model.replaceChildren(...models.map((name, index) => h("option", { value: String(index) }, name)));
      model.value = String(Math.min(Math.max(0, current.model), Math.max(0, models.length - 1)));
      // With no microphone named there is nothing to emulate and nothing to swap.
      model.disabled = models.length === 0;
      swap.toggleAttribute("data-unavailable", models.length === 0);
      swap.setAttribute("aria-pressed", String(current.swap));
    });
    // The panel offers emulation only on a preamp set to Mic, and so does this.
    this.watch(() => {
      const mic = inputs.preamp(i).value.type === 0;
      target.disabled = !mic;
      swap.toggleAttribute("data-unavailable", !mic);
      if (!mic) model.disabled = true;
    });

    return h("div", { class: "mic" }, h("span", { class: "name" }, label), target, model, swap);
  }

  #digital(inputs: InputsModel, group: DigitalGroup, i: number, enabled: () => boolean): HTMLElement {
    const label = `${group.label.replace(/ in$/, "")} ${i + 1}`;
    const gainOf = inputs.digitalGain(group.kind, i);
    // Only inputs this app sets can link (a read-only gain has nothing to send).
    const link = group.editable ? linkButton((fn) => this.watch(fn), group.kind, inputs.deviceId, i, `${group.kind}-link-${i}`) : undefined;
    const heading = h("span", { class: "cell-head" }, h("span", { class: "label" }, label), link);
    const value = h("span", { class: "value" });
    const fill = h("div", { class: "fill" });
    // Read-only gains (the Quadro's panel never sets them) get the same bar, without the slider role or input.
    const gain = group.editable
      ? h("div", { class: "gain", role: "slider", tabindex: 0, "aria-label": `${label} gain`, "aria-valuemin": DIGITAL_GAIN.min, "aria-valuemax": DIGITAL_GAIN.max, "data-testid": `${group.kind}-gain-${i}` }, fill, value)
      : h("div", { class: "gain readonly", "aria-label": `${label} gain`, title: "This device's panel does not set this gain", "data-testid": `${group.kind}-gain-${i}` }, fill, value);
    if (group.editable) bindControl(gain, { axis: "x", min: DIGITAL_GAIN.min, max: DIGITAL_GAIN.max, up: 1, page: 3, reset: 0, get: () => gainOf.peek() ?? 0, set: (v) => inputs.setDigitalGain(group.kind, i, v), enabled });
    this.watch(() => {
      const g = gainOf.value ?? 0;
      // A report outside the range (the loopback's test pattern) must not push the bar out of its cell.
      const shown = Math.min(DIGITAL_GAIN.max, Math.max(DIGITAL_GAIN.min, g));
      fill.style.width = `${((shown - DIGITAL_GAIN.min) / (DIGITAL_GAIN.max - DIGITAL_GAIN.min)) * 100}%`;
      value.textContent = gainOf.value === undefined ? "—" : formatGain(g);
      gain.setAttribute("aria-valuenow", String(g));
    });
    return h("div", { class: "cell" }, heading, gain);
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-inputs": GaInputs;
  }
}
