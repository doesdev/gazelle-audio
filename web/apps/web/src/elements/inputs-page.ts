// <ga-inputs device-id="…">: every hardware input on a device, shown whether or not a mixer channel
// uses it. Preamps have type, gain, 48V, phase invert and the HPF the device reports; digital inputs
// have gain, editable where the device's own panel sets it. 48V turns on only with a confirming
// second click or Ctrl/Cmd+click, as the Quadro panel guards it; turning it off is one click.

import { h } from "../core/dom.ts";
import { DIGITAL_GAIN, GAIN_RANGE, PREAMP_TYPES, type DigitalGroup, type InputsModel, type PreampType } from "../store/inputs.ts";
import { bindControl, type ControlOptions } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { LINK_STYLES, linkBar, linkButton } from "./link-bar.ts";
import { polarPlot, POLAR_PLOT_STYLES, stereoOrientation, type PlotHead } from "./polar-plot.ts";
import { href } from "./router.ts";

/** How long a first 48V click waits for its confirmation. */
const ARM_MS = 3000;

const testId = <E extends Element>(element: E, id: string): E => {
  element.setAttribute("data-testid", id);
  return element;
};

const formatGain = (db: number) => `${db > 0 ? "+" : ""}${db} dB`;

/** A microphone or emulation's name in a list, saying so when the device's licence does not cover it. */
const unlicensed = (name: string, licensed: boolean) => (licensed ? name : `${name} (not licensed)`);

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
      .mic { display: grid; grid-template-columns: minmax(64px, 88px) minmax(0, 1fr) minmax(0, 2fr) auto auto; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 3px; background: var(--ga-surface-raised); }
      .mic select { min-width: 0; min-height: 24px; }
      .models { display: flex; min-width: 0; gap: 6px; }
      .models select { flex: 1; min-width: 0; }
      .head { display: flex; flex: 1; min-width: 0; align-items: center; gap: 4px; }
      .head select { flex: 1; min-width: 0; }
      .stereo { display: flex; min-width: 0; align-items: center; gap: 6px; }
      .stereo[hidden] { display: none; }
      .stereo select { min-width: 0; }
      .head-name { font-size: 10px; color: var(--ga-text-muted); white-space: nowrap; }
      /* Two heads stack, one line each, beside the plot that draws them both. */
      .models[data-heads="2"] { flex-direction: column; }
      .models[data-heads="2"] .head-name { min-width: 48px; }
      /* With both heads in one plot, each head's name carries its tone as the plot's legend. */
      .head-name[data-tone]::before { content: ""; display: inline-block; width: 6px; height: 6px; margin-right: 3px; border-radius: 50%; vertical-align: 1px; }
      .head-name[data-tone="0"]::before { background: var(--ga-accent); }
      .head-name[data-tone="1"]::before { background: var(--ga-text-primary); }
      ${POLAR_PLOT_STYLES}
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
          h("p", { class: "note-inline" }, "For Antelope's own microphones on a preamp set to Mic. An Edge Duo covers two preamps and an Edge Quadro four, and picking one links them. A polar pattern runs from omni through cardioid to figure-8, as far as the emulated microphone allows, and its plot draws a lobe of inverted polarity dashed. A stereo technique also wants the top head turned 90°, which is yours to do: the Edge Quadro's plot shows the heads as the technique wants them turned, not as they sit."),
        );
    void inputs.loadEmulations();
    void inputs.loadLicence();

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

  /**
   * One preamp's row in the Mic emulation section. A microphone that covers more than one preamp
   * shows on the first of them and hides the rest, with an emulation for each of its heads.
   */
  #emulation(inputs: InputsModel, i: number): HTMLElement {
    const state = inputs.emulation(i);
    const row = h("div", { class: "mic", "data-testid": `mic-row-${i}` });
    const name = h("span", { class: "name" });
    const target = h(
      "select",
      { "aria-label": `Preamp ${i + 1} microphone`, "data-testid": `mic-target-${i}`, "on:change": () => inputs.setEmulationTarget(i, Number(target.value)) },
      inputs.micTargets.map((t) => h("option", { value: String(t.value) }, t.name)),
    );
    const models = h("span", { class: "models" });
    const preset = h("select", { class: "preset", "aria-label": `Preamp ${i + 1} stereo technique`, "data-testid": `mic-preset-${i}`, "on:change": () => inputs.setEmulationPreset(i, Number(preset.value)) });
    // The technique, and beside it both heads' patterns in one plot, which is how a technique reads best.
    const stereo = h("span", { class: "stereo", hidden: "" }, preset);
    const swap = h("button", {
      type: "button",
      class: "swap",
      "data-control": "",
      "data-testid": `mic-swap-${i}`,
      "aria-label": `Preamp ${i + 1} swap the microphone's front and rear membranes`,
      title: "Swap the microphone's front and rear membranes",
      "on:click": () => inputs.setEmulationSwap(i, !state.peek().swap),
    }, "Swap");

    this.watch(() => {
      const current = state.value;
      const span = inputs.emulationSpan(current.target);
      const first = Math.floor(i / span) * span;
      // Only the microphone's first preamp carries its controls; the ones it covers step aside.
      row.hidden = first !== i;
      if (row.hidden) return;
      const channels = inputs.emulationChannels(i, current.target);
      // The row shows the whole microphone, so it follows every channel of it: a head's model and
      // polar pattern live on their own channel's state, which this one would not otherwise read.
      for (const channel of channels) inputs.emulation(channel).value;
      name.textContent = span === 1 ? `Preamp ${i + 1}` : `Preamps ${first + 1}–${first + span}`;
      if (this.root.activeElement !== target) target.value = String(current.target);
      // What the device's licence does not cover stays listed, greyed, as the panel greys it; one
      // the device is already on still shows as selected.
      for (const t of inputs.micTargets) {
        const option = target.options[t.value];
        if (option === undefined) continue;
        option.disabled = !t.licensed;
        option.textContent = unlicensed(t.name, t.licensed);
      }

      const catalogue = inputs.emulationModels(current.target);
      const heads = catalogue.length === 0 ? [] : inputs.emulationHeads(i, current.target);
      // A microphone with two heads draws them together beside its technique; one head draws its own.
      const overlaid = heads.length > 1;
      models.setAttribute("data-heads", String(heads.length));
      models.replaceChildren(
        ...heads.map((head) => {
          const which = head.name === "" ? "" : `-${head.name.toLowerCase()}`;
          const select = h(
            "select",
            { "aria-label": `Preamp ${i + 1} ${head.name === "" ? "emulation" : `${head.name} head emulation`}`, "data-testid": `mic-model-${i}${which}`, "on:change": () => inputs.setEmulationModel(head.channel, Number(select.value)) },
            catalogue.map((label, index) => {
              const licensed = inputs.emulationLicensed(current.target, index);
              return h("option", { value: String(index), ...(licensed ? {} : { disabled: "" }) }, unlicensed(label, licensed));
            }),
          );
          select.value = String(Math.min(Math.max(0, inputs.emulation(head.channel).peek().model), catalogue.length - 1));
          const parts: Element[] = [...(head.name === "" ? [] : [h("span", { class: "head-name" }, head.name)]), select];
          // The polar pattern is the head's, and only some emulations have one to point.
          const pattern = inputs.emulationPattern(head.channel);
          if (pattern !== undefined) {
            const steps = pattern.steps ?? Array.from({ length: 11 }, (_, k) => ({ value: Math.round(pattern.min + (k / 10) * (pattern.max - pattern.min)), label: "" }));
            const polar = h(
              "select",
              { class: "polar", "aria-label": `Preamp ${i + 1} ${head.name === "" ? "polar pattern" : `${head.name} head polar pattern`}`, "data-testid": `mic-pattern-${i}${which}`, "on:change": () => inputs.setEmulationPattern(head.channel, Number(polar.value)) },
              steps.map((step) => h("option", { value: String(step.value) }, step.label === "" ? String(step.value) : step.label)),
            );
            polar.value = String(pattern.value);
            polar.disabled = steps.length < 2;
            parts.push(polar);
            if (overlaid) parts[0]?.setAttribute("data-tone", String(heads.indexOf(head)));
            else parts.push(testId(polarPlot([{ angle: pattern.angle }], `Polar pattern: ${pattern.label}`), `mic-plot-${i}`));
          }
          return parts.length === 1 ? select : h("span", { class: "head" }, ...parts);
        }),
      );

      // A stereo technique belongs to the microphone, not a head, so it sits beside the swap.
      const presets = inputs.emulationPresets(i);
      if (presets.length === 0) stereo.hidden = true;
      else {
        stereo.hidden = false;
        preset.replaceChildren(...presets.map((p) => h("option", { value: String(p.value), ...(p.available ? {} : { disabled: "" }) }, p.name)));
        preset.value = String(inputs.emulationPreset(i));
        // Only the plot is replaced: moving the select would take focus from it mid-pick.
        stereo.querySelector(".polar-plot")?.remove();
        stereo.append(...this.#overlay(inputs, i, heads, presets[inputs.emulationPreset(i)]?.name ?? "None"));
      }
      // With no microphone named there is nothing to emulate and nothing to swap.
      if (heads.length === 0) models.replaceChildren(h("select", { "aria-label": `Preamp ${i + 1} emulation`, "data-testid": `mic-model-${i}`, disabled: "" }));
      swap.toggleAttribute("data-unavailable", span === 1);
      swap.setAttribute("aria-pressed", String(current.swap));
      // The panel offers emulation only on a preamp set to Mic, and so does this. A microphone on
      // more than one preamp needs all of them on Mic, since they are all the one microphone.
      const onMic = channels.every((channel) => inputs.preamp(channel).value.type === 0);
      target.disabled = !onMic;
      for (const select of models.querySelectorAll("select")) select.disabled = !onMic || heads.length === 0;
      for (const polar of models.querySelectorAll<HTMLSelectElement>(".polar")) if (polar.options.length < 2) polar.disabled = true;
      preset.disabled = !onMic;
      for (const plot of row.querySelectorAll(".polar-plot")) plot.setAttribute("aria-disabled", String(!onMic));
      if (!onMic) swap.toggleAttribute("data-unavailable", true);
    });

    row.replaceChildren(name, target, models, stereo, swap);
    return row;
  }

  /**
   * Both heads of a microphone in one plot, Bottom in the accent and Top in the text colour. Under a
   * stereo technique they are drawn turned as it wants them, and the label says so, since the app
   * cannot see how the heads really sit. Empty when neither head has a pattern to draw.
   */
  #overlay(inputs: InputsModel, i: number, heads: readonly { name: string; channel: number }[], technique: string): SVGSVGElement[] {
    const patterns = heads.map((head) => inputs.emulationPattern(head.channel));
    if (patterns.every((pattern) => pattern === undefined)) return [];
    const [bottom, top] = patterns;
    const turns = bottom !== undefined && top !== undefined ? stereoOrientation(technique, [bottom.angle, top.angle]) : [0, 0];
    const drawn: PlotHead[] = [];
    const named: string[] = [];
    patterns.forEach((pattern, which) => {
      if (pattern === undefined) return;
      drawn.push({ angle: pattern.angle, rotation: turns[which] ?? 0, tone: which });
      named.push(`${heads[which]?.name ?? ""} ${pattern.label}`);
    });
    const turned = turns.some((turn) => turn !== 0) ? `, drawn turned as ${technique} wants the heads, not as they sit` : "";
    return [testId(polarPlot(drawn, `Polar patterns: ${named.join(", ")}${turned}`, 36), `mic-plot-${i}`)];
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
