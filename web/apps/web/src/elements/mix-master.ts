// <ga-mix-master device-id="…" mix="0">: one mix's master. Its head names the mix and shows where
// the mix plays: a chip per output pair it feeds (× stops it) and a menu to add one, from hardware
// outputs to the computer's record inputs. Below is the mix's master strip. The mixer page matches
// the head's height to the channel heads (--channel-head), so faders line up.

import { h } from "../core/dom.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";

export class GaMixMaster extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
      .head {
        display: grid;
        align-content: start;
        gap: 3px;
        box-sizing: border-box;
        height: var(--channel-head, auto);
        min-width: 0;
        margin: 0;
        padding: 4px 3px;
        overflow: auto;
        border: 0;
        border-radius: 3px;
        background: var(--ga-surface-raised);
      }
      .head input, .head select { width: 100%; min-width: 0; min-height: 20px; padding: 0 2px; font-size: 10px; }
      .caption { min-height: 1.4em; font-size: 9px; color: var(--ga-text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
      .chips { display: flex; flex-wrap: wrap; gap: 2px; }
      .chip { display: inline-flex; align-items: center; max-width: 100%; padding-left: 4px; border-radius: 2px; background: var(--ga-accent); color: var(--ga-accent-text); font-size: 9px; font-weight: 700; }
      .chip span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .chip button { min-width: 0; min-height: 14px; padding: 0 3px; border: 0; background: transparent; color: inherit; font-size: 10px; }
      .empty { font-size: 9px; color: var(--ga-text-muted); }
      .mono { min-width: 0; min-height: 18px; padding: 0 4px; font-size: 10px; font-weight: 700; }
      .mono[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .strip-slot { display: flex; flex: 1; min-height: 0; }
      .strip-slot ga-strip { flex: 1; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const mix = Number(this.getAttribute("mix") ?? "0");
    const channels = store.channels(deviceId);
    const outputs = channels.mixOutputs(mix);

    const name = h("input", { type: "text", "aria-label": `Mix ${mix + 1} name`, placeholder: `Mix ${mix + 1}`, "data-testid": `mix-name-${mix}` });
    commitOnEnter(name, (value) => channels.renameMix(mix, value.trim()), () => channels.layout.peek().mixes[mix]?.name ?? "");
    const chips = h("div", { class: "chips", "data-testid": `mix-outputs-${mix}` });
    const add = h("select", {
      "aria-label": `Add an output for mix ${mix + 1}`,
      "data-testid": `mix-add-output-${mix}`,
      // A menu of actions rather than a value: each wheel step would add an output (P73).
      "data-no-wheel": true,
      "on:change": () => {
        const [destination, channel] = add.value.split(":").map(Number);
        add.value = "";
        if (destination !== undefined && channel !== undefined && !Number.isNaN(destination)) void channels.setMixOutput(mix, { destination, channel }, true);
      },
    });
    // Mono (P57): the app centres the mix's pans and restores them after; the device has no switch for it.
    const mono = h(
      "button",
      { type: "button", class: "mono", "data-testid": `mix-mono-${mix}`, "aria-label": `Mix ${mix + 1} mono`, title: "Sum this mix to mono: pans every channel to centre, and restores the pans when turned off", "on:click": () => channels.setMono(mix, !channels.isMono(mix)) },
      "Mono",
    );
    this.watch(() => mono.setAttribute("aria-pressed", String(channels.isMono(mix))));
    const head = h("fieldset", { class: "head", "aria-label": `Mix ${mix + 1}` }, name, mono, h("span", { class: "caption" }, "Outputs"), chips, add);
    const stripSlot = h("div", { class: "strip-slot" });
    this.root.replaceChildren(head, stripSlot);

    this.watch(() => {
      const mixName = channels.mixName(mix);
      if (this.root.activeElement !== name) name.value = channels.layout.value.mixes[mix]?.name ?? "";
      const playing = outputs.value;
      chips.replaceChildren(
        ...(playing.length === 0
          ? [h("span", { class: "empty" }, "Not playing anywhere")]
          : playing.map((pair) =>
              h(
                "span",
                { class: "chip", title: pair.label },
                h("span", {}, pair.label),
                h("button", { type: "button", "aria-label": `Stop ${mixName} feeding ${pair.label}`, "on:click": () => void channels.setMixOutput(mix, pair, false) }, "×"),
              ),
            )),
      );
      const fed = new Set(playing.map((p) => `${p.destination}:${p.channel}`));
      add.replaceChildren(h("option", { value: "" }, "+ Output…"), ...channels.outputPairs().filter((p) => !fed.has(`${p.destination}:${p.channel}`)).map((p) => h("option", { value: `${p.destination}:${p.channel}` }, p.label)));
      add.value = "";
    });
    this.watch(() => {
      head.disabled = !store.connected.value;
    });
    let rendered = "";
    this.watch(() => {
      const label = channels.mixName(mix);
      if (label === rendered) return;
      rendered = label;
      stripSlot.replaceChildren(h("ga-strip", { "device-id": deviceId, mixer: String(mix), strip: "master", label }));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-mix-master": GaMixMaster;
  }
}
