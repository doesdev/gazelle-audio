// <ga-effects device-id="…">: a device's effect chains and its reverb (EffectsModel; spec
// 2026-09-17-effects-and-reverb). Each chain is a card: what routing feeds it, its effects in order by
// name and instance, and per effect a Process / Bypass pair that shows neither until the effect's
// parameters are read or this app sets one. Each card also has an "Add effect" menu of the types this
// model has, those with no free instance left greyed rather than left out, and each effect can be moved
// earlier or later or removed: every one of those writes the whole chain with set_afx_order, packed from
// the first slot, and reads it back. Reordering has never been tried on a device, which its buttons say.
// Links are shown, not changed. The reverb has on/off and
// level as controls and its other parameters as the panel displays them; the Quadro adds its reverb
// returns into mixes 1-2 and sends from mix 1's channels. Everything is read once when the page opens
// (P80); Read from device reads again.
// Choosing an effect opens its editor below the chains: its own parameters, read once for that instance,
// a control per parameter as the vendor panel's code describes it (a bar for a range, a switch, a menu, or
// a button per bit), each set back to the panel's starting value on double-click, and the effect's bypass.
// An effect set band by band (the Studio+ Equalizer) shows a group of controls per band.

import { h } from "../core/dom.ts";
import { effect as effectOf, signal, untracked } from "../core/signal.ts";
import type { EffectParameter } from "../store/effect-parameters.ts";
import { bandKey, formatParameter, formatReverbLevel, formatRoomSize, REVERB_LEVEL_MAX, REVERB_LEVEL_MIN, REVERB_LEVEL_UNITY, REVERB_RETURN_MAX, REVERB_SEND_MAX, shownParameters, type EffectChain, type EffectSlot, type EffectsModel } from "../store/effects.ts";
import { formatPan, meterDeflection, PAN_CENTRE, PAN_MAX, PAN_MIN, panAtPosition } from "../store/mixer.ts";
import { formatVolume } from "../store/outputs.ts";
import { meterGradient } from "../themes/theme.ts";
import { bindControl, levelReset } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { animateMeter, METER_FLOOR } from "./meter-motion.ts";

/** The Quadro panel's names for its two reverb returns, by the mix they feed. */
const RETURN_NAMES = ["Mix 1 (Monitor/HP1)", "Mix 2 (HP2)"] as const;

/** Why a parameter has no control, for the editor's note. */
const HIDDEN_REASONS: Record<NonNullable<EffectParameter["hidden"]>, string> = {
  sidechain: "Its sidechain source is chosen from routing in the vendor panel and is kept as the device reports it.",
  link: "Its link setting follows the chains' stereo link and is kept as the device reports it.",
  internal: "Settings the vendor panel does not show are kept as the device reports them.",
  unused: "Settings no amp model uses are kept as the device reports them.",
  model: "Settings the chosen amp model does not use, and those no model uses, are kept as the device reports them.",
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
      .slot { display: grid; grid-template-columns: 1.5em minmax(0, 1fr) auto; align-items: center; gap: 4px 6px; min-height: 26px; padding: 2px 4px; border-radius: 2px; background: var(--ga-surface-inset); }
      /* The effect's own meter, on its own row under the name so it never squeezes the name out. */
      .effect-meter { grid-row: 2; grid-column: 2 / -1; display: grid; grid-template-columns: minmax(0, 1fr) 6px auto; align-items: center; gap: 6px; }
      .effect-meter .bar { position: relative; height: 5px; overflow: hidden; border-radius: 1px; background: var(--ga-meter-background); }
      .effect-meter .bar .gradient { position: absolute; inset: 0; background: var(--effect-meter-gradient, var(--ga-accent)); }
      .effect-meter .bar .mask { position: absolute; top: 0; bottom: 0; right: 0; width: 100%; background: var(--ga-meter-background); }
      .effect-meter .bar .mask::after { content: ""; position: absolute; top: 0; bottom: 0; left: -2px; width: 2px; background: var(--ga-text-primary); opacity: 0.35; filter: blur(1.5px); }
      .effect-meter .bar .peak-mark { position: absolute; top: 0; bottom: 0; width: 2px; margin-left: -1px; background: var(--ga-text-primary); opacity: 0.85; box-shadow: 0 0 4px var(--ga-text-primary); }
      .effect-meter .bar .peak-mark[hidden] { display: none; }
      .effect-meter .clip { width: 6px; height: 10px; padding: 0; border: 0; border-radius: 1px; background: var(--ga-meter-background); }
      .effect-meter .clip[data-on] { background: var(--ga-meter-clip); cursor: pointer; }
      .effect-meter .reduction { font-size: 10px; color: var(--ga-text-muted); font-variant-numeric: tabular-nums; white-space: nowrap; }
      .slot-tools { display: flex; flex-wrap: wrap; justify-content: flex-end; align-items: center; gap: 4px; }
      .slot-tools button { min-width: 22px; min-height: 20px; padding: 0 4px; font-size: 11px; line-height: 1; }
      .add { max-width: 100%; min-width: 0; font-size: 11px; }
      .slot .position { grid-row: 1; grid-column: 1; font-size: 10px; color: var(--ga-text-muted); text-align: right; }
      .slot .effect { grid-row: 1; grid-column: 2; }
      .slot .slot-tools { grid-row: 1; grid-column: 3; }
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
      .bands { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 200px), 1fr)); gap: 8px 12px; }
      .band { display: grid; align-content: start; gap: 4px; padding: 6px 8px; border-radius: 3px; background: var(--ga-surface-inset); min-width: 0; }
      .band h3 { margin: 0 0 2px; font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 12px; font-weight: 600; color: var(--ga-text-secondary); }
      .band .param { grid-template-columns: minmax(58px, 72px) minmax(0, 1fr); }
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

    // The parameter catalogue is a chunk of its own, fetched as the page opens rather than with the
    // app: an editor is built once it is here (store/effect-parameters.ts).
    void effects.loadCatalogue();
    // Read once when the page opens, and again once the connection or the device has come back.
    this.watch(() => {
      if (store.connected.value && effects.needsRead.value) untracked(() => void effects.readOnce());
    });
    // Follow the effect meters while the page is open, and nowhere else: about 125 reports a second.
    this.watch(() => effects.activate());
    this.watch(() => {
      this.style.setProperty("--effect-meter-gradient", meterGradient(store.theme.value.meter.gradient, undefined, "to right", (db) => meterDeflection(-db)));
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
      "data-explain": "effects.read",
      "on:click": async () => {
        if (!(await effects.load()) && !store.server.peek().dry_run) store.reportError("The effects could not all be read from the device.");
      },
    }, "Read from device");
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent", "data-explain": "page.last-sent" });
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
    let shown: { chain: number; type: number; inst: number } | undefined;
    this.watch(() => {
      const chosen = this.#chosen.value;
      const slot = chosen === undefined ? undefined : effects.chains.value?.[chosen.chain]?.slots.find((s) => s.type === chosen.type && s.inst === chosen.inst);
      // Nothing is built, and nothing read from the device, until the catalogue is here: reading it
      // here makes this run again when it arrives.
      if (!effects.catalogueReady) {
        editor.replaceChildren();
        shown = undefined;
        return;
      }
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
      h("section", {}, h("h2", { "data-explain": "effects.chains" }, "Effect chains"), h("p", { class: "note" }, "Each chain processes what routing sends to its AFX IN channel and returns on AFX OUT. Choose an effect to see and change its settings. Bypass is per effect; neither button shows lit until the effect's settings are read or you set one. Adding, removing and moving an effect writes the whole chain and reads it back; the device switches an effect on as it is added and off as it goes. Inserting and removing one effect have been checked on a Quadro; moving one, several effects in a chain and the Studio+ have not."), chains),
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

  /**
   * The effect whose editor is open: its chain and which instance it is rather than its slot, so that the
   * editor follows it when the chain is reordered and closes when it is removed.
   */
  readonly #chosen = signal<{ chain: number; type: number; inst: number } | undefined>(undefined);

  #chain(effects: EffectsModel, index: number, chain: EffectChain | undefined, deviceId: string, destination: number, disposers: (() => void)[], chosen: { chain: number; type: number; inst: number } | undefined): HTMLElement {
    const store = useStore();
    const topology = store.topology(deviceId);
    const name = chain?.name ?? `AFX IN ${index + 1}`;
    const source = h("span", { class: "source", "data-testid": `chain-source-${index}`, "data-explain": "effects.chain-source" });
    if (destination >= 0) {
      disposers.push(
        this.#effect(() => {
          const slot = store.routing(deviceId).destination(destination).value?.[index];
          const input = slot === undefined ? undefined : topology?.inputs[slot.source];
          source.textContent = slot === undefined ? "" : input === undefined || input.type === "MUTE" ? "← nothing routed" : `← ${input.name}${input.channels > 1 ? ` ${slot.channel + 1}` : ""}`;
        }),
      );
    }
    const link = h("span", { class: "link", "data-testid": `chain-link-${index}`, "data-explain": "effects.chain-link", title: chain === undefined ? "" : `Linked with AFX IN ${chain.partner + 1}`, hidden: chain?.linked !== true }, "LINK");
    const head = h("div", { class: "chain-head" }, h("span", { class: "name" }, name), link, source);

    if (chain === undefined || !chain.known) {
      return h("div", { class: "chain", "data-testid": `chain-${index}` }, head, h("span", { class: "empty" }, "Not read from the device"));
    }

    const rows = chain.slots.map((slot) => {
      const bypass = effects.bypass(slot.type, slot.inst);
      const move = (by: number, where: string, arrow: string, testId: string) =>
        h("button", { type: "button", class: "move", "data-testid": `${testId}-${index}-${slot.position}`, "data-explain": by < 0 ? "effects.move-up" : "effects.move-down", "aria-label": `Move ${slot.name} ${slot.inst + 1} ${where} in ${name}`, title: `Move ${where}. Reordering a chain has never been tried on a device.`, "on:click": () => effects.moveEffect(index, slot.position, by) }, arrow);
      const up = move(-1, "earlier", "\u2191", "move-up");
      const down = move(1, "later", "\u2193", "move-down");
      const remove = h("button", { type: "button", class: "remove", "data-testid": `remove-${index}-${slot.position}`, "data-explain": "effects.remove", "aria-label": `Remove ${slot.name} ${slot.inst + 1} from ${name}`, title: `Remove ${slot.name} ${slot.inst + 1} from the chain. The device switches the effect off as it goes.`, "on:click": () => effects.removeEffect(index, slot.position) }, "\u2715");
      const process = h("button", { type: "button", class: "process", "data-testid": `active-${index}-${slot.position}`, "data-explain": "effects.on", "aria-label": `Process with ${slot.name} ${slot.inst + 1}`, "on:click": () => effects.setBypass(index, slot.position, false) }, "On");
      const off = h("button", { type: "button", class: "bypass", "data-testid": `bypass-${index}-${slot.position}`, "data-explain": "effects.bypass", "aria-label": `Bypass ${slot.name} ${slot.inst + 1}`, "on:click": () => effects.setBypass(index, slot.position, true) }, "Bypass");
      const meter = this.#slotMeter(deviceId, index, slot, disposers);
      disposers.push(
        this.#effect(() => {
          const state = bypass.value;
          process.setAttribute("aria-pressed", String(state === false));
          off.setAttribute("aria-pressed", String(state === true));
          const on = store.connected.value;
          process.disabled = off.disabled = remove.disabled = !on;
          up.disabled = !on || slot.position === 0;
          down.disabled = !on || slot.position >= chain.slots.length - 1;
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
            "data-explain": "effects.effect",
            "data-explain-name": `${slot.name} #${slot.inst + 1}`,
            "aria-expanded": String(chosen?.chain === index && chosen.type === slot.type && chosen.inst === slot.inst),
            title: `Settings of ${slot.name} ${slot.inst + 1} (effect type ${slot.type})`,
            "on:click": () => {
              const open = this.#chosen.peek();
              this.#chosen.value = open?.chain === index && open.type === slot.type && open.inst === slot.inst ? undefined : { chain: index, type: slot.type, inst: slot.inst };
            },
          },
          slot.name,
          " ",
          h("span", { class: "instance" }, `#${slot.inst + 1}`),
        ),
        meter,
        h("span", { class: "slot-tools" }, h("span", { class: "pair", role: "group", "aria-label": `${slot.name} bypass` }, process, off), up, down, remove),
      );
    });
    const empty = chain.slots.length === 0;
    const all = (bypassed: boolean, label: string, testId: string) =>
      h("button", { type: "button", "data-control": "", "data-testid": `${testId}-${index}`, "data-explain": bypassed ? "effects.bypass-all" : "effects.process-all", ...(empty ? { "data-empty": "" } : {}), disabled: empty || !store.connected.peek(), "on:click": () => effects.setChainBypass(index, bypassed) }, label);
    return h(
      "div",
      { class: "chain", "data-testid": `chain-${index}` },
      head,
      empty ? h("span", { class: "empty" }, "No effects") : h("ol", { class: "slots" }, rows),
      this.#add(effects, index, name, disposers),
      h("div", { class: "chain-tools" }, all(true, "Bypass all", "chain-bypass-all"), all(false, "Process all", "chain-enable-all")),
    );
  }

  /**
   * A chain's "Add effect" menu: every effect type this model has, the ones with no free instance left
   * greyed rather than left out, so it is plain that the device has them and none is free. Choosing one
   * puts it after what is already in the chain, on the lowest free instance.
   */
  #add(effects: EffectsModel, index: number, name: string, disposers: (() => void)[]): HTMLElement {
    const store = useStore();
    const select = h("select", {
      class: "add",
      "data-testid": `chain-add-${index}`,
      "data-explain": "effects.add",
      "aria-label": `Add an effect to ${name}`,
      // The wheel steps a select and sends (P73); this menu acts on being chosen, not on being scrolled past.
      "data-no-wheel": true,
      "on:change": (event: Event) => {
        const menu = event.target as HTMLSelectElement;
        const type = Number(menu.value);
        menu.value = "";
        if (Number.isInteger(type) && type > 0) effects.addEffect(index, type);
      },
    });
    disposers.push(
      this.#effect(() => {
        const offers = effects.offers(index);
        const connected = store.connected.value;
        select.replaceChildren(
          h("option", { value: "" }, "Add effect\u2026"),
          ...offers.map((offer) =>
            h(
              "option",
              {
                value: String(offer.type),
                disabled: offer.unavailable !== undefined,
                title: offer.unavailable ?? (offer.counted ? `${offer.free} free` : `${offer.free} free, worked out from the chains this app has read rather than counted by the device`),
              },
              offer.name,
            ),
          ),
        );
        select.value = "";
        select.disabled = !connected || offers.every((offer) => offer.unavailable !== undefined);
      }),
    );
    return select;
  }

  /** The editor for the effect in one slot: its bypass, and a control per parameter once read. */
  #editor(effects: EffectsModel, chain: number, slot: EffectSlot, disposers: (() => void)[]): HTMLElement {
    const store = useStore();
    const description = effects.description(slot.type);
    const close = h("button", { type: "button", "data-testid": "editor-close", "aria-label": "Close the effect's settings", "data-explain": "effects.editor-close", "on:click": () => (this.#chosen.value = undefined) }, "Close");
    const bypass = effects.bypass(slot.type, slot.inst);
    const process = h("button", { type: "button", class: "process", "data-testid": "editor-active", "data-explain": "effects.on", "aria-label": `Process with ${slot.name} ${slot.inst + 1}`, "on:click": () => effects.setBypass(chain, slot.position, false) }, "On");
    const off = h("button", { type: "button", class: "bypass", "data-testid": "editor-bypass", "data-explain": "effects.bypass", "aria-label": `Bypass ${slot.name} ${slot.inst + 1}`, "on:click": () => effects.setBypass(chain, slot.position, true) }, "Bypass");
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
      h("h2", { "data-explain": "effects.editor", "data-explain-name": `${slot.name} #${slot.inst + 1}` }, `${slot.name} #${slot.inst + 1}`),
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
    const every = [...description.parameters, ...(description.bands?.bands.flat() ?? [])];
    // The amp's model-dependent settings include those no model uses: one sentence says both.
    const kinds = new Set(every.flatMap((p) => (p.hidden === undefined ? [] : [p.hidden])));
    if (kinds.has("model")) kinds.delete("unused");
    const hidden = [...kinds].map((kind) => HIDDEN_REASONS[kind]);
    const unitless = every.some((p) => p.control === "range" && p.unit === undefined && p.scale === undefined && p.kilo !== true);
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
    if (description.bands !== undefined) {
      section.append(this.#bands(effects, chain, slot, description.bands.bands, enabled, disposers));
      return section;
    }
    const params = h("div", { class: "params", "data-testid": "effect-params" });
    const applyEnabled = () => {
      const on = enabled();
      for (const button of params.querySelectorAll<HTMLButtonElement | HTMLSelectElement>("button, select")) button.disabled = !on;
      for (const bar of params.querySelectorAll('[role="slider"]')) bar.setAttribute("aria-disabled", String(!on));
    };
    // The controls shown, rebuilt only when what picks them changes (the Guitar Amp's model: each model's
    // view in the panel has its own knobs and switches).
    const layouts = description.layouts;
    const starting = Object.fromEntries(description.parameters.map((p) => [p.name, p.default ?? 0]));
    let shownKey: number | undefined | null = null;
    let controlDisposers: (() => void)[] = [];
    disposers.push(() => controlDisposers.forEach((dispose) => dispose()));
    disposers.push(
      this.#effect(() => {
        const current = values.value?.values ?? starting;
        const key = layouts === undefined ? undefined : current[layouts.by];
        if (key === shownKey) return;
        shownKey = key;
        untracked(() => {
          controlDisposers.forEach((dispose) => dispose());
          controlDisposers = [];
          params.replaceChildren(
            ...shownParameters(description, current).map((parameter) => {
              const get = () => values.peek()?.values[parameter.name] ?? parameter.default ?? 0;
              const set = (value: number) => void effects.setParameter(chain, slot.position, parameter.name, value);
              const reset = () => {
                if (enabled()) effects.resetParameter(chain, slot.position, parameter.name);
              };
              const control = this.#parameterControl(parameter, get, set, reset, enabled, controlDisposers, () => values.value?.values[parameter.name] ?? parameter.default ?? 0);
              return h("div", { class: "param" }, h("span", { class: "label", title: parameter.label }, parameter.label), control);
            }),
          );
          if (layouts !== undefined && key !== undefined && !layouts.models.has(key)) {
            params.append(h("p", { class: "note", "data-testid": "editor-layout-note" }, `The device reports a ${description.parameters.find((p) => p.name === layouts.by)?.label.toLowerCase() ?? layouts.by} (${key}) the vendor panel does not offer, so only the settings every one has are shown.`));
          }
          applyEnabled();
        });
      }),
    );
    section.append(params);
    disposers.push(
      this.#effect(() => {
        void values.value;
        void store.connected.value;
        applyEnabled();
      }),
    );
    return section;
  }

  /**
   * An effect set band by band: a group per band, each control sending only its band. A band's gain is off
   * (0, not changeable) while its filter is a pass filter, as the panel greys it.
   */
  #bands(effects: EffectsModel, chain: number, slot: EffectSlot, bands: readonly (readonly EffectParameter[])[], enabled: () => boolean, disposers: (() => void)[]): HTMLElement {
    const store = useStore();
    const values = effects.parameters(slot.type, slot.inst);
    const refresh: (() => void)[] = [];
    const groups = bands.map((band, b) => {
      const controls = band
        .filter((parameter) => parameter.control !== undefined)
        .map((parameter) => {
          const key = bandKey(parameter.name, b);
          const off = parameter.offWhen;
          const isOff = () => off !== undefined && off.values.includes(values.peek()?.values[bandKey(off.name, b)] ?? Number.NaN);
          const bandEnabled = () => enabled() && !isOff();
          const get = () => values.peek()?.values[key] ?? parameter.default ?? 0;
          const set = (value: number) => void effects.setParameter(chain, slot.position, parameter.name, value, b);
          const reset = () => {
            if (bandEnabled()) effects.resetParameter(chain, slot.position, parameter.name, b);
          };
          const label = `Band ${b + 1} ${parameter.label.toLowerCase()}`;
          const control = this.#parameterControl({ ...parameter, label }, get, set, reset, bandEnabled, disposers, () => values.value?.values[key] ?? parameter.default ?? 0, `param-${parameter.name}-${b}`);
          refresh.push(() => {
            const on = bandEnabled();
            if (control instanceof HTMLButtonElement || control instanceof HTMLSelectElement) control.disabled = !on;
            if (control.getAttribute("role") === "slider") control.setAttribute("aria-disabled", String(!on));
            control.title = isOff() ? `Off while band ${b + 1} is a pass filter` : "";
          });
          return h("div", { class: "param" }, h("span", { class: "label", title: parameter.label }, parameter.label), control);
        });
      return h("section", { class: "band", "data-testid": `band-${b}`, "aria-label": `Band ${b + 1}` }, h("h3", { "data-explain": "effects.band", "data-explain-name": `Band ${b + 1}` }, `Band ${b + 1}`), ...controls);
    });
    disposers.push(
      this.#effect(() => {
        void values.value;
        void store.connected.value;
        refresh.forEach((apply) => apply());
      }),
    );
    return h("div", { class: "bands", "data-testid": "effect-bands" }, groups);
  }

  #parameterControl(parameter: EffectParameter, get: () => number, set: (value: number) => void, reset: () => void, enabled: () => boolean, disposers: (() => void)[], current: () => number, testId = `param-${parameter.name}`): HTMLElement {
    const label = parameter.label;
    switch (parameter.control) {
      case "switch": {
        const button = h("button", { type: "button", class: "toggle", "data-testid": testId, "aria-label": label, "data-explain": "effects.param-switch", "data-explain-name": label, "on:click": () => set(get() === 0 ? 1 : 0), "on:dblclick": reset });
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
        const select = h("select", { "data-testid": testId, "aria-label": label, "data-explain": "effects.param-menu", "data-explain-name": label, "on:change": (event: Event) => set(Number((event.target as HTMLSelectElement).value)), "on:dblclick": reset }, (parameter.options ?? []).map(([value, text]) => h("option", { value: String(value) }, text)));
        disposers.push(
          this.#effect(() => {
            select.value = String(current());
          }),
        );
        return select;
      }
      case "bits": {
        const buttons = (parameter.options ?? []).map(([bit, text]) =>
          h("button", { type: "button", "data-testid": `${testId}-${Math.log2(bit)}`, "aria-label": `${label} ${text}`, "data-explain": "effects.param-bits", "data-explain-name": `${label}: ${text}`, "on:click": () => set(get() ^ bit) }, text),
        );
        const group = h("div", { class: "bits", role: "group", "data-testid": testId, "aria-label": label, "data-explain": "effects.param-bits", "data-explain-name": label, "on:dblclick": reset }, buttons);
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
        const bar = barControl(testId, label, "effects.param-range");
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

  /**
   * One effect's meter: its peak on the mixer's scale with the same ballistics (P86) and clip light
   * (P88), and the gain reduction the device reports beside it.
   *
   * The gain reduction is shown in the device's own steps. The vendor panel reads it as dB for some
   * effects and as quarter-dB for others, depending on the meter widget each effect's view uses, so
   * a single figure in dB would be wrong for half of them; what it means is a hardware check
   * (`reference/devices.md`).
   */
  #slotMeter(deviceId: string, chain: number, slot: EffectSlot, disposers: (() => void)[]): HTMLElement {
    const store = useStore();
    const meter = store.effectMeter(deviceId, chain, slot.position);
    const reduction = h("span", { class: "reduction" });
    if (meter === undefined) {
      reduction.textContent = "no meter";
      return h("span", { class: "effect-meter", title: "The device reports no meter for this effect", "data-explain": "effects.no-meter" }, reduction);
    }
    const mask = h("div", { class: "mask" });
    const peakMark = h("div", { class: "peak-mark", hidden: "" });
    const bar = h("div", { class: "bar", "aria-hidden": "true" }, h("div", { class: "gradient" }), mask, peakMark);
    const clip = h("button", { type: "button", class: "clip", "data-testid": `clip-${chain}-${slot.position}`, "data-explain": "effects.clip", "aria-label": `${slot.name} ${slot.inst + 1} clip; select to clear`, "on:click": () => meter.clearClip() });
    disposers.push(
      animateMeter(meter.level, (motion) => {
        mask.style.width = `${100 - meterDeflection(motion.level)}%`;
        peakMark.hidden = motion.peak >= METER_FLOOR;
        peakMark.style.left = `${meterDeflection(motion.peak)}%`;
      }),
      this.#effect(() => {
        clip.toggleAttribute("data-on", meter.clipped.value);
        const gr = meter.reduction.value;
        reduction.textContent = gr === undefined ? "GR …" : `GR ${gr}`;
      }),
    );
    return h("span", { class: "effect-meter", "data-testid": `meter-${chain}-${slot.position}`, "data-explain": "effects.meter", title: "The effect's level, dB below full scale, and the gain reduction the device reports, in its own steps" }, bar, clip, reduction);
  }

  #reverb(effects: EffectsModel, enabled: () => boolean): HTMLElement {
    const store = useStore();
    const on = h("button", { type: "button", class: "toggle on", "data-testid": "reverb-on", "aria-label": "Reverb on", "data-explain": "reverb.on", "on:click": () => effects.setReverbOn(!(effects.reverb.peek()?.on ?? false)) }, "On");
    const level = barControl("reverb-level", "Reverb level", "reverb.level");
    bindControl(level.element, {
      axis: "x",
      min: REVERB_LEVEL_MIN,
      max: REVERB_LEVEL_MAX,
      up: 1,
      page: 5,
      // A quarter of unity, 2.5, is -20 dB; the nearest step the level has is 3, -18 dB.
      reset: 3,
      level: levelReset(store.doubleClickUnity, (fn) => this.watch(fn), REVERB_LEVEL_UNITY, "-18 dB, the nearest it has to -20 dB"),
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
      params.replaceChildren(...shown.flatMap(([term, detail]) => [h("dt", {}, term), h("dd", {}, h("span", { class: "readout", "data-explain": "reverb.param", "data-explain-name": term }, detail))]));
    });
    return h(
      "section",
      {},
      h("h2", { "data-explain": "reverb.heading" }, "Reverb"),
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
      const level = barControl(`return-level-${mix}`, `Reverb return to ${name}`, "reverb.return-level");
      bindControl(level.element, {
        axis: "x",
        min: REVERB_RETURN_MAX,
        max: 0,
        up: -1,
        page: 6,
        // The steps have no known scale: twenty below full is -20 dB if they are decibels, as the
        // mixer's are.
        reset: 20,
        level: levelReset(store.doubleClickUnity, (fn) => this.watch(fn), 0, "20 steps below full, about -20 dB if the steps are decibels", "full"),
        get: () => effects.returns.peek()?.entries[mix]?.level ?? 0,
        set: (v) => effects.setReturn(mix, { level: v }),
        enabled: () => enabled() && effects.returns.peek() !== undefined,
      });
      const mute = h("button", { type: "button", class: "toggle mute", "data-testid": `return-mute-${mix}`, "data-explain": "reverb.return-mute", "aria-label": `Mute the reverb return to ${name}`, "on:click": () => effects.setReturn(mix, { mute: !(effects.returns.peek()?.entries[mix]?.mute ?? false) }) }, "Mute");
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
    return h("section", {}, h("h2", { "data-explain": "reverb.returns" }, "Reverb returns"), h("p", { class: "note" }, "How much reverb each mix gets back. The vendor panel shows no scale for these, so they are shown in its own steps, 0 at full to 90; not yet checked on the device."), h("div", { class: "rows" }, rows));
  }

  #sends(effects: EffectsModel, enabled: () => boolean): HTMLElement {
    const rows = Array.from({ length: 16 }, (_, i) => {
      const channel = i + 1;
      const level = barControl(`send-level-${channel}`, `Reverb send from mix 1 channel ${channel}`, "reverb.send-level");
      bindControl(level.element, {
        axis: "x",
        min: REVERB_SEND_MAX,
        max: 0,
        up: -1,
        page: 6,
        // A send's double-click is off, so it never adds reverb (the user, 2026-09-18).
        reset: REVERB_SEND_MAX,
        level: levelReset(useStore().doubleClickUnity, (fn) => this.watch(fn), 0, "off"),
        get: () => effects.sends.peek()?.entries[i]?.level ?? REVERB_SEND_MAX,
        set: (v) => effects.setSend(channel, { level: v }),
        enabled: () => enabled() && effects.sends.peek() !== undefined,
      });
      const pan = barControl(`send-pan-${channel}`, `Reverb send pan, mix 1 channel ${channel}`, "reverb.send-pan");
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
    return h("section", {}, h("h2", { "data-explain": "reverb.sends" }, "Reverb sends"), h("p", { class: "note" }, "How much of each of mix 1's channels 1 to 16 goes to the reverb, and where it sits in it, as the vendor panel offers."), h("div", { class: "sends" }, rows));
  }
}

/** A horizontal value bar with a text readout, as the Outputs page's volumes; a centred one (a pan) fills from the middle. */
function barControl(testId: string, label: string, explain: string): { element: HTMLElement; show(fraction: number, text: string, now: number, centred?: boolean): void } {
  const fill = h("div", { class: "fill" });
  const value = h("span", { class: "value" });
  const element = h("div", { class: "bar-control", role: "slider", tabindex: 0, "aria-label": label, "data-testid": testId, "data-explain": explain, "data-explain-name": label }, fill, value);
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
