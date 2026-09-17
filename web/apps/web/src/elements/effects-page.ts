// <ga-effects device-id="…">: a device's effect chains and its reverb (EffectsModel; spec
// 2026-09-17-effects-and-reverb). Each chain is a card: what routing feeds it, its effects in order by
// name and instance, and per effect a Process / Bypass pair that shows neither until the effect's
// parameters are read or this app sets one. Links are shown, not changed. The reverb has on/off and
// level as controls and its other parameters as the panel displays them; the Quadro adds its reverb
// returns into mixes 1-2 and sends from mix 1's channels. Everything is read once when the page opens
// (P80); Read from device reads again.
// Choosing an effect opens its editor below the chains: its own parameters, read once for that instance,
// a control per parameter as the vendor panel's code describes it (a bar for a range, a switch, a menu, or
// a button per bit), each set back to the panel's starting value on double-click, and the effect's bypass.

import { h } from "../core/dom.ts";
import { effect as effectOf, signal, untracked } from "../core/signal.ts";
import type { EffectParameter } from "../store/effect-parameters.ts";
import { formatParameter, formatReverbLevel, formatRoomSize, REVERB_LEVEL_MAX, REVERB_LEVEL_MIN, REVERB_LEVEL_UNITY, REVERB_RETURN_MAX, REVERB_SEND_MAX, type EffectChain, type EffectSlot, type EffectsModel } from "../store/effects.ts";
import { formatPan, PAN_CENTRE, PAN_MAX, PAN_MIN, panAtPosition } from "../store/mixer.ts";
import { formatVolume } from "../store/outputs.ts";
import { bindControl } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";

/** The Quadro panel's names for its two reverb returns, by the mix they feed. */
const RETURN_NAMES = ["Mix 1 (Monitor/HP1)", "Mix 2 (HP2)"] as const;

/** Why a parameter has no control, for the editor's note. */
const HIDDEN_REASONS: Record<NonNullable<EffectParameter["hidden"]>, string> = {
  sidechain: "Its sidechain source is chosen from routing in the vendor panel and is kept as the device reports it.",
  link: "Its link setting follows the chains' stereo link and is kept as the device reports it.",
  internal: "Settings the vendor panel does not show are kept as the device reports them.",
  unused: "Settings no amp model uses are kept as the device reports them.",
  model: "Its mode switches work differently for each amp model and are kept as the device reports them.",
};

export class GaEffects extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 12px; }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .spacer { flex: 1; }
      .note { margin: 0; font-size: 11px; color: var(--ga-text-muted); }
      .last-sent { display: flex; min-width: 0; max-width: 100%; font-size: 11px; white-space: nowrap; }
      .last-sent code { min-width: 0; overflow: hidden; text-overflow: ellipsis; font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      h2 { margin: 8px 0 6px; }
      .chains { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 260px), 1fr)); gap: 6px; }
      .chain { display: flex; flex-direction: column; gap: 4px; padding: 6px 8px; border-radius: 3px; background: var(--ga-surface-raised); min-width: 0; }
      .chain-head { display: flex; align-items: center; gap: 6px; min-width: 0; }
      .name { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; }
      .source { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 11px; color: var(--ga-text-secondary); }
      .link { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-accent-text); background: var(--ga-accent); }
      .slots { display: grid; gap: 2px; margin: 0; padding: 0; list-style: none; }
      .slot { display: grid; grid-template-columns: 1.5em minmax(0, 1fr) auto; align-items: center; gap: 6px; min-height: 26px; padding: 0 4px; border-radius: 2px; background: var(--ga-surface-inset); }
      .slot .position { font-size: 10px; color: var(--ga-text-muted); text-align: right; }
      .slot .effect { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .slot .instance { color: var(--ga-text-muted); font-size: 11px; }
      .empty { font-size: 11px; color: var(--ga-text-muted); }
      .pair { display: flex; }
      .pair button { min-width: 0; padding: 0 6px; font-size: 10px; font-weight: 700; }
      .pair button:first-child { border-radius: 3px 0 0 3px; }
      .pair button:last-child { border-radius: 0 3px 3px 0; border-left: 0; }
      .pair .process[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .pair .bypass[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .chain-tools { display: flex; gap: 4px; justify-content: flex-end; }
      .chain-tools button { min-height: 20px; padding: 0 6px; font-size: 10px; }
      .rows { display: grid; gap: 6px; max-width: 640px; }
      .row { display: grid; grid-template-columns: minmax(96px, 150px) minmax(0, 1fr) auto; align-items: center; gap: 10px; padding: 6px 8px; border-radius: 3px; background: var(--ga-surface-raised); }
      .slot .effect { justify-self: start; max-width: 100%; min-height: 22px; padding: 0 4px; border: 0; border-radius: 2px; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
      .slot .effect:hover, .slot .effect[aria-expanded="true"] { background: var(--ga-surface-raised); }
      .slot .effect[aria-expanded="true"] { outline: 1px solid var(--ga-accent); }
      .editor { display: flex; flex-direction: column; gap: 8px; padding: 8px 10px; border-radius: 3px; background: var(--ga-surface-raised); min-width: 0; }
      .editor-head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; min-width: 0; }
      .editor-head h2 { margin: 0; font-size: 15px; }
      .editor-head .where { font-size: 11px; color: var(--ga-text-secondary); }
      .params { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 250px), 1fr)); gap: 6px 12px; }
      .param { display: grid; grid-template-columns: minmax(70px, 110px) minmax(0, 1fr); align-items: center; gap: 8px; min-height: 26px; min-width: 0; }
      .param .label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; color: var(--ga-text-secondary); }
      .param select { min-width: 0; max-width: 100%; }
      .param .toggle { justify-self: start; }
      .bits { display: flex; flex-wrap: wrap; gap: 4px; }
      .bits button { min-width: 44px; font-size: 11px; }
      .bits button[aria-pressed="true"], .param .toggle[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .sends { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 300px), 1fr)); gap: 6px; }
      .send { display: grid; grid-template-columns: 3.5em minmax(0, 2fr) minmax(0, 1fr); align-items: center; gap: 6px; padding: 4px 8px; border-radius: 3px; background: var(--ga-surface-raised); }
      .bar-control {
        position: relative;
        height: 22px;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 3px;
        background: var(--ga-surface-inset);
        cursor: ew-resize;
        touch-action: none;
        outline: none;
      }
      .bar-control:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .bar-control .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
      .bar-control .value { position: absolute; inset: 0; font-size: 11px; line-height: 20px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
      .bar-control[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .toggle { min-width: 44px; font-size: 11px; font-weight: 700; }
      .on[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .fields { max-width: 640px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const topology = store.topology(deviceId);
    if (topology === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so its effects are not known.`));
      return;
    }
    const effects = store.effects(deviceId);
    const connected = () => store.connected.peek();

    // Read once when the page opens, and again once the connection or the device has come back.
    this.watch(() => {
      if (store.connected.value && effects.needsRead.value) untracked(() => void effects.readOnce());
    });
    const destination = topology.outputs.findIndex((output) => output.type === "AFX_IN");
    if (destination >= 0) {
      this.watch(() => {
        if (store.routesToRead(deviceId, [destination])) untracked(() => void store.readRoutes(deviceId, [destination]));
      });
    }

    const reload = h("button", {
      type: "button",
      "data-control": "",
      "data-testid": "effects-read",
      "on:click": async () => {
        if (!(await effects.load()) && !store.server.peek().dry_run) store.reportError("The effects could not all be read from the device.");
      },
    }, "Read from device");
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent" });
    const note = h("p", { class: "note", "data-testid": "effects-note" });

    const chains = h("div", { class: "chains" });
    this.watch(() => {
      const list = effects.chains.value;
      const chosen = this.#chosen.value;
      const disposers: (() => void)[] = [];
      const count = effects.family === "quadro" ? 6 : 16;
      chains.replaceChildren(
        ...Array.from({ length: count }, (_, index) => this.#chain(effects, index, list?.[index], deviceId, destination, disposers, chosen)),
      );
      return () => disposers.forEach((dispose) => dispose());
    });

    // The chosen effect's editor, rebuilt when the chains are read again or another effect is chosen.
    const editor = h("div", { class: "editor-slot" });
    let shown: { chain: number; position: number } | undefined;
    this.watch(() => {
      const chosen = this.#chosen.value;
      const slot = chosen === undefined ? undefined : effects.chains.value?.[chosen.chain]?.slots.find((s) => s.position === chosen.position);
      if (chosen === undefined || slot === undefined) {
        editor.replaceChildren();
        shown = undefined;
        return;
      }
      const disposers: (() => void)[] = [];
      editor.replaceChildren(untracked(() => this.#editor(effects, chosen.chain, slot, disposers)));
      // Newly chosen, not merely read again: bring it into view, since on a phone (or with the Studio+'s
      // sixteen chains) it opens well below the effect that was chosen.
      if (shown !== chosen) editor.firstElementChild?.scrollIntoView?.({ block: "nearest" });
      shown = chosen;
      return () => disposers.forEach((dispose) => dispose());
    });

    const reverb = this.#reverb(effects, connected);
    const returnsAndSends = effects.hasReturnsAndSends ? [this.#returns(effects, connected), this.#sends(effects, connected)] : [h("p", { class: "note" }, "On the Studio+ the reverb send is each channel's Send on the Mixer page (the panel has it on mix 1), and the return is the reverb level above.")];

    this.root.replaceChildren(
      h("div", { class: "bar" }, reload, h("span", { class: "spacer" }), lastSent),
      note,
      h("section", {}, h("h2", {}, "Effect chains"), h("p", { class: "note" }, "Each chain processes what routing sends to its AFX IN channel and returns on AFX OUT. Choose an effect to see and change its settings. Bypass is per effect; neither button shows lit until the effect's settings are read or you set one. Adding, moving and removing effects are not in this version."), chains),
      editor,
      reverb,
      ...returnsAndSends,
    );

    this.watch(() => {
      const chainsRead = effects.chains.value;
      const reverbRead = effects.reverb.value;
      if (effects.needsRead.value && chainsRead === undefined) note.textContent = store.connected.value ? "Reading the effects from the device…" : "";
      else if (chainsRead !== undefined && chainsRead.every((c) => !c.known) && reverbRead?.known !== true) note.textContent = "Nothing was read from the device (dry run): chains are unknown, and the reverb controls start at the panel's values.";
      else if (chainsRead === undefined || reverbRead === undefined) note.textContent = "Some of the effects could not be read from the device; Read from device tries again.";
      else note.textContent = "";
    });
    this.watch(() => {
      const sent = store.lastSent.value;
      if (sent === undefined || sent.deviceId !== deviceId) {
        lastSent.textContent = store.server.value.dry_run ? "Dry run: nothing is written to the device" : "";
        return;
      }
      lastSent.replaceChildren(`${sent.dryRun ? "Dry run, would send" : "Sent"} ${sent.command}: `, h("code", { title: sent.hex }, sent.hex));
    });
    this.watch(() => {
      const on = store.connected.value;
      void effects.chains.value;
      for (const button of this.root.querySelectorAll<HTMLButtonElement>("button[data-control]")) button.disabled = !on || button.hasAttribute("data-empty");
      for (const control of this.root.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!on));
    });
  }

  /** The effect whose editor is open, by chain and slot position. */
  readonly #chosen = signal<{ chain: number; position: number } | undefined>(undefined);

  #chain(effects: EffectsModel, index: number, chain: EffectChain | undefined, deviceId: string, destination: number, disposers: (() => void)[], chosen: { chain: number; position: number } | undefined): HTMLElement {
    const store = useStore();
    const topology = store.topology(deviceId);
    const name = chain?.name ?? `AFX IN ${index + 1}`;
    const source = h("span", { class: "source", "data-testid": `chain-source-${index}` });
    if (destination >= 0) {
      disposers.push(
        this.#effect(() => {
          const slot = store.routing(deviceId).destination(destination).value?.[index];
          const input = slot === undefined ? undefined : topology?.inputs[slot.source];
          source.textContent = slot === undefined ? "" : input === undefined || input.type === "MUTE" ? "← nothing routed" : `← ${input.name}${input.channels > 1 ? ` ${slot.channel + 1}` : ""}`;
        }),
      );
    }
    const link = h("span", { class: "link", "data-testid": `chain-link-${index}`, title: chain === undefined ? "" : `Linked with AFX IN ${chain.partner + 1}`, hidden: chain?.linked !== true }, "LINK");
    const head = h("div", { class: "chain-head" }, h("span", { class: "name" }, name), link, source);

    if (chain === undefined || !chain.known) {
      return h("div", { class: "chain", "data-testid": `chain-${index}` }, head, h("span", { class: "empty" }, "Not read from the device"));
    }

    const rows = chain.slots.map((slot) => {
      const bypass = effects.bypass(slot.type, slot.inst);
      const process = h("button", { type: "button", class: "process", "data-testid": `active-${index}-${slot.position}`, "aria-label": `Process with ${slot.name} ${slot.inst + 1}`, "on:click": () => effects.setBypass(index, slot.position, false) }, "On");
      const off = h("button", { type: "button", class: "bypass", "data-testid": `bypass-${index}-${slot.position}`, "aria-label": `Bypass ${slot.name} ${slot.inst + 1}`, "on:click": () => effects.setBypass(index, slot.position, true) }, "Bypass");
      disposers.push(
        this.#effect(() => {
          const state = bypass.value;
          process.setAttribute("aria-pressed", String(state === false));
          off.setAttribute("aria-pressed", String(state === true));
          process.disabled = off.disabled = !store.connected.value;
        }),
      );
      return h(
        "li",
        { class: "slot", "data-testid": `slot-${index}-${slot.position}` },
        h("span", { class: "position" }, String(slot.position + 1)),
        h(
          "button",
          {
            type: "button",
            class: "effect",
            "data-testid": `edit-${index}-${slot.position}`,
            "aria-expanded": String(chosen?.chain === index && chosen.position === slot.position),
            title: `Settings of ${slot.name} ${slot.inst + 1} (effect type ${slot.type})`,
            "on:click": () => {
              const open = this.#chosen.peek();
              this.#chosen.value = open?.chain === index && open.position === slot.position ? undefined : { chain: index, position: slot.position };
            },
          },
          slot.name,
          " ",
          h("span", { class: "instance" }, `#${slot.inst + 1}`),
        ),
        h("span", { class: "pair", role: "group", "aria-label": `${slot.name} bypass` }, process, off),
      );
    });
    const empty = chain.slots.length === 0;
    const all = (bypassed: boolean, label: string, testId: string) =>
      h("button", { type: "button", "data-control": "", "data-testid": `${testId}-${index}`, ...(empty ? { "data-empty": "" } : {}), disabled: empty || !store.connected.peek(), "on:click": () => effects.setChainBypass(index, bypassed) }, label);
    return h(
      "div",
      { class: "chain", "data-testid": `chain-${index}` },
      head,
      empty ? h("span", { class: "empty" }, "No effects") : h("ol", { class: "slots" }, rows),
      h("div", { class: "chain-tools" }, all(true, "Bypass all", "chain-bypass-all"), all(false, "Process all", "chain-enable-all")),
    );
  }

  /** The editor for the effect in one slot: its bypass, and a control per parameter once read. */
  #editor(effects: EffectsModel, chain: number, slot: EffectSlot, disposers: (() => void)[]): HTMLElement {
    const store = useStore();
    const description = effects.description(slot.type);
    const close = h("button", { type: "button", "data-testid": "editor-close", "aria-label": "Close the effect's settings", "on:click": () => (this.#chosen.value = undefined) }, "Close");
    const bypass = effects.bypass(slot.type, slot.inst);
    const process = h("button", { type: "button", class: "process", "data-testid": "editor-active", "aria-label": `Process with ${slot.name} ${slot.inst + 1}`, "on:click": () => effects.setBypass(chain, slot.position, false) }, "On");
    const off = h("button", { type: "button", class: "bypass", "data-testid": "editor-bypass", "aria-label": `Bypass ${slot.name} ${slot.inst + 1}`, "on:click": () => effects.setBypass(chain, slot.position, true) }, "Bypass");
    disposers.push(
      this.#effect(() => {
        process.setAttribute("aria-pressed", String(bypass.value === false));
        off.setAttribute("aria-pressed", String(bypass.value === true));
        process.disabled = off.disabled = !store.connected.value;
      }),
    );
    const head = h(
      "div",
      { class: "editor-head" },
      h("h2", {}, `${slot.name} #${slot.inst + 1}`),
      h("span", { class: "where" }, `AFX IN ${chain + 1}, slot ${slot.position + 1}`),
      h("span", { class: "spacer" }),
      h("span", { class: "pair", role: "group", "aria-label": `${slot.name} bypass` }, process, off),
      close,
    );
    const note = h("p", { class: "note", "data-testid": "editor-note" });
    const section = h("section", { class: "editor", "data-testid": "effect-editor", "aria-label": `${slot.name} ${slot.inst + 1} settings` }, head, note);
    if (description === undefined) {
      note.textContent = `This effect's settings are not editable here: ${effects.unsupportedReason(slot.type)}`;
      return section;
    }

    const values = effects.parameters(slot.type, slot.inst);
    const hidden = [...new Set(description.parameters.flatMap((p) => (p.hidden === undefined ? [] : [HIDDEN_REASONS[p.hidden]])))];
    const unitless = description.parameters.some((p) => p.control === "range" && p.unit === undefined && p.scale === undefined);
    disposers.push(
      this.#effect(() => {
        const read = values.value;
        const parts = [
          read === undefined ? (store.connected.value ? "Reading the settings from the device…" : "Not read from the device.") : read.known ? "" : "Nothing was read from the device (dry run): the settings start at the vendor panel's values.",
          unitless ? "Values without a unit are the device's own steps: the vendor panel draws their scales in its artwork, which is not copied here." : "",
          ...hidden,
        ];
        note.textContent = parts.filter((part) => part !== "").join(" ");
      }),
    );
    untracked(() => void effects.readParameters(slot.type, slot.inst));

    const enabled = () => store.connected.peek() && values.peek() !== undefined;
    const params = h(
      "div",
      { class: "params" },
      description.parameters.filter((p) => p.control !== undefined).map((parameter) => {
        const get = () => values.peek()?.values[parameter.name] ?? parameter.default ?? 0;
        const set = (value: number) => void effects.setParameter(chain, slot.position, parameter.name, value);
        const reset = () => {
          if (enabled()) effects.resetParameter(chain, slot.position, parameter.name);
        };
        const control = this.#parameterControl(parameter, get, set, reset, enabled, disposers, () => values.value?.values[parameter.name] ?? parameter.default ?? 0);
        return h("div", { class: "param" }, h("span", { class: "label", title: parameter.label }, parameter.label), control);
      }),
    );
    section.append(params);
    disposers.push(
      this.#effect(() => {
        void values.value;
        const on = store.connected.value && values.value !== undefined;
        for (const button of params.querySelectorAll<HTMLButtonElement | HTMLSelectElement>("button, select")) button.disabled = !on;
        for (const bar of params.querySelectorAll('[role="slider"]')) bar.setAttribute("aria-disabled", String(!on));
      }),
    );
    return section;
  }

  #parameterControl(parameter: EffectParameter, get: () => number, set: (value: number) => void, reset: () => void, enabled: () => boolean, disposers: (() => void)[], current: () => number): HTMLElement {
    const testId = `param-${parameter.name}`;
    const label = parameter.label;
    switch (parameter.control) {
      case "switch": {
        const button = h("button", { type: "button", class: "toggle", "data-testid": testId, "aria-label": label, "on:click": () => set(get() === 0 ? 1 : 0), "on:dblclick": reset });
        disposers.push(
          this.#effect(() => {
            const value = current();
            button.setAttribute("aria-pressed", String(value !== 0));
            button.textContent = formatParameter(parameter, value);
          }),
        );
        return button;
      }
      case "menu": {
        const select = h("select", { "data-testid": testId, "aria-label": label, "on:change": (event: Event) => set(Number((event.target as HTMLSelectElement).value)), "on:dblclick": reset }, (parameter.options ?? []).map(([value, text]) => h("option", { value: String(value) }, text)));
        disposers.push(
          this.#effect(() => {
            select.value = String(current());
          }),
        );
        return select;
      }
      case "bits": {
        const buttons = (parameter.options ?? []).map(([bit, text]) =>
          h("button", { type: "button", "data-testid": `${testId}-${Math.log2(bit)}`, "aria-label": `${label} ${text}`, "on:click": () => set(get() ^ bit) }, text),
        );
        const group = h("div", { class: "bits", role: "group", "data-testid": testId, "aria-label": label, "on:dblclick": reset }, buttons);
        disposers.push(
          this.#effect(() => {
            const value = current();
            (parameter.options ?? []).forEach(([bit], i) => buttons[i]?.setAttribute("aria-pressed", String((value & bit) !== 0)));
            group.title = formatParameter(parameter, value);
          }),
        );
        return group;
      }
      default: {
        const min = parameter.min ?? 0;
        const max = parameter.max ?? 0;
        const bar = barControl(testId, label);
        bindControl(bar.element, { axis: "x", min, max, up: 1, page: Math.max(1, Math.round((max - min) / 20)), reset: parameter.default ?? min, get, set, enabled });
        disposers.push(
          this.#effect(() => {
            const value = current();
            // Filled from the middle only when zero is the middle (a symmetrical range such as -5..5).
            bar.show(max === min ? 0 : (value - min) / (max - min), formatParameter(parameter, value), value, min < 0 && min === -max);
          }),
        );
        return bar.element;
      }
    }
  }

  /** An effect disposed with the chain list rather than the page, since the list is rebuilt on every read. */
  #effect(fn: () => void): () => void {
    return untracked(() => effectOf(fn));
  }

  #reverb(effects: EffectsModel, enabled: () => boolean): HTMLElement {
    const store = useStore();
    const on = h("button", { type: "button", class: "toggle on", "data-testid": "reverb-on", "aria-label": "Reverb on", "on:click": () => effects.setReverbOn(!(effects.reverb.peek()?.on ?? false)) }, "On");
    const level = barControl("reverb-level", "Reverb level");
    bindControl(level.element, {
      axis: "x",
      min: REVERB_LEVEL_MIN,
      max: REVERB_LEVEL_MAX,
      up: 1,
      page: 5,
      reset: REVERB_LEVEL_UNITY,
      get: () => effects.reverb.peek()?.level ?? REVERB_LEVEL_UNITY,
      set: (v) => effects.setReverbLevel(v),
      enabled: () => enabled() && effects.reverb.peek() !== undefined,
    });
    const params = h("dl", { class: "fields", "data-testid": "reverb-params" });
    this.watch(() => {
      const config = effects.reverb.value;
      on.setAttribute("aria-pressed", String(config?.on ?? false));
      on.disabled = config === undefined || !store.connected.value;
      const value = config?.level ?? REVERB_LEVEL_UNITY;
      level.show((value - REVERB_LEVEL_MIN) / (REVERB_LEVEL_MAX - REVERB_LEVEL_MIN), formatReverbLevel(value), value);
      const shown: [string, string][] =
        config === undefined
          ? []
          : [
              ["Room size", formatRoomSize(config.roomSize)],
              ["Reverb time", String(config.reverbTime)],
              ["Pre-delay", String(config.predelay)],
              ["Colour", String(config.color)],
              ["Early reflections", String(config.earlyRefGain)],
              ["Late reflection delay", String(config.lateRefDelay)],
              ["Richness", String(config.richness)],
            ];
      params.replaceChildren(...shown.flatMap(([term, detail]) => [h("dt", {}, term), h("dd", {}, h("span", { class: "readout" }, detail))]));
    });
    return h(
      "section",
      {},
      h("h2", {}, "Reverb"),
      h(
        "div",
        { class: "rows" },
        h("div", { class: "row" }, h("span", { class: "name" }, "Reverb"), h("span", {}), on),
        h("div", { class: "row" }, h("span", { class: "name" }, "Level"), level.element, h("span", {})),
      ),
      h("p", { class: "note" }, "One reverb for the whole device. Its other settings are shown as the device reports them, in the vendor panel's units; changing them is not in this version."),
      params,
    );
  }

  #returns(effects: EffectsModel, enabled: () => boolean): HTMLElement {
    const store = useStore();
    const rows = RETURN_NAMES.map((name, mix) => {
      const level = barControl(`return-level-${mix}`, `Reverb return to ${name}`);
      bindControl(level.element, {
        axis: "x",
        min: REVERB_RETURN_MAX,
        max: 0,
        up: -1,
        page: 6,
        reset: 0,
        get: () => effects.returns.peek()?.entries[mix]?.level ?? 0,
        set: (v) => effects.setReturn(mix, { level: v }),
        enabled: () => enabled() && effects.returns.peek() !== undefined,
      });
      const mute = h("button", { type: "button", class: "toggle mute", "data-testid": `return-mute-${mix}`, "aria-label": `Mute the reverb return to ${name}`, "on:click": () => effects.setReturn(mix, { mute: !(effects.returns.peek()?.entries[mix]?.mute ?? false) }) }, "Mute");
      this.watch(() => {
        const entry = effects.returns.value?.entries[mix];
        const value = entry?.level ?? 0;
        // The panel shows no scale: 0 is the loudest and 90 the quietest, so it reads as steps down.
        level.show((REVERB_RETURN_MAX - value) / REVERB_RETURN_MAX, value === 0 ? "full" : `-${value} steps`, -value);
        mute.setAttribute("aria-pressed", String(entry?.mute ?? false));
        mute.disabled = entry === undefined || !store.connected.value;
      });
      return h("div", { class: "row" }, h("span", { class: "name" }, name), level.element, mute);
    });
    return h("section", {}, h("h2", {}, "Reverb returns"), h("p", { class: "note" }, "How much reverb each mix gets back. The vendor panel shows no scale for these, so they are shown in its own steps, 0 at full to 90; not yet checked on the device."), h("div", { class: "rows" }, rows));
  }

  #sends(effects: EffectsModel, enabled: () => boolean): HTMLElement {
    const rows = Array.from({ length: 16 }, (_, i) => {
      const channel = i + 1;
      const level = barControl(`send-level-${channel}`, `Reverb send from mix 1 channel ${channel}`);
      bindControl(level.element, {
        axis: "x",
        min: REVERB_SEND_MAX,
        max: 0,
        up: -1,
        page: 6,
        reset: REVERB_SEND_MAX,
        get: () => effects.sends.peek()?.entries[i]?.level ?? REVERB_SEND_MAX,
        set: (v) => effects.setSend(channel, { level: v }),
        enabled: () => enabled() && effects.sends.peek() !== undefined,
      });
      const pan = barControl(`send-pan-${channel}`, `Reverb send pan, mix 1 channel ${channel}`);
      bindControl(pan.element, {
        axis: "x",
        min: PAN_MIN,
        max: PAN_MAX,
        up: 1,
        page: 5,
        reset: PAN_CENTRE,
        valueAt: panAtPosition,
        get: () => effects.sends.peek()?.entries[i]?.pan ?? PAN_CENTRE,
        set: (v) => effects.setSend(channel, { pan: v }),
        enabled: () => enabled() && effects.sends.peek() !== undefined,
      });
      this.watch(() => {
        const entry = effects.sends.value?.entries[i];
        const value = entry?.level ?? REVERB_SEND_MAX;
        level.show((REVERB_SEND_MAX - value) / REVERB_SEND_MAX, formatVolume(value), -value);
        const p = entry?.pan ?? PAN_CENTRE;
        pan.show((p - PAN_MIN) / (PAN_MAX - PAN_MIN), formatPan(p), p - PAN_CENTRE, true);
      });
      return h("div", { class: "send" }, h("span", { class: "label" }, `Ch ${channel}`), level.element, pan.element);
    });
    return h("section", {}, h("h2", {}, "Reverb sends"), h("p", { class: "note" }, "How much of each of mix 1's channels 1–16 goes to the reverb, and where it sits in it, as the vendor panel offers."), h("div", { class: "sends" }, rows));
  }
}

/** A horizontal value bar with a text readout, as the Outputs page's volumes; a centred one (a pan) fills from the middle. */
function barControl(testId: string, label: string): { element: HTMLElement; show(fraction: number, text: string, now: number, centred?: boolean): void } {
  const fill = h("div", { class: "fill" });
  const value = h("span", { class: "value" });
  const element = h("div", { class: "bar-control", role: "slider", tabindex: 0, "aria-label": label, "data-testid": testId }, fill, value);
  return {
    element,
    show(fraction, text, now, centred = false) {
      const f = Math.min(1, Math.max(0, fraction));
      fill.style.left = `${(centred ? Math.min(f, 0.5) : 0) * 100}%`;
      fill.style.width = `${(centred ? Math.abs(f - 0.5) : f) * 100}%`;
      value.textContent = text;
      element.setAttribute("aria-valuenow", String(now));
      element.setAttribute("aria-valuetext", text);
    },
  };
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-effects": GaEffects;
  }
}
