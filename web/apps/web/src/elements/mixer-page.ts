// <ga-mixer device-id="…">: one device's mixer, built from the channels the user made (plan
// 2026-09-16). Channels scroll horizontally, followed by a "+" to add one; the masters of the mixes
// in use sit on the right. A device with no layout imports one from its routing when the page
// opens. The device meters one mix at a time, chosen in the bar. In dry run it shows the bytes of
// the last command sent.

import { h } from "../core/dom.ts";
import { meterDeflection } from "../store/mixer.ts";
import { STRIP_WIDTH_MAX, STRIP_WIDTH_MIN } from "../store/preferences.ts";
import { meterGradient } from "../themes/theme.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href } from "./router.ts";

export class GaMixer extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 8px; flex: 1; min-height: 0; }
      /* Channel width: a caption, an Auto | Fixed segmented control and an inset px field, all one height. */
      .width { display: flex; align-items: center; gap: 6px; }
      .caption { font-size: 11px; color: var(--ga-text-secondary); }
      .segmented { display: flex; }
      .segmented button {
        min-height: 26px;
        padding: 0 10px;
        border-radius: 0;
        color: var(--ga-text-secondary);
        font-size: 11px;
        font-weight: 600;
      }
      .segmented button + button { margin-left: -1px; }
      .segmented button:first-child { border-radius: 3px 0 0 3px; }
      .segmented button:last-child { border-radius: 0 3px 3px 0; }
      .segmented button[aria-pressed="true"] { position: relative; border-color: var(--ga-accent); background: var(--ga-accent); color: var(--ga-accent-text); }
      .px-field {
        display: flex;
        align-items: center;
        height: 26px;
        padding-right: 7px;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 3px;
        background: var(--ga-surface-inset);
        cursor: text;
      }
      .px-field:focus-within { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .px-field:has(input:disabled) { opacity: 0.5; cursor: not-allowed; }
      .px-field input {
        width: 3.4em;
        min-height: 24px;
        padding: 0 2px 0 6px;
        border: 0;
        background: transparent;
        text-align: right;
        font-variant-numeric: tabular-nums;
        appearance: textfield;
      }
      .px-field input:disabled { background: transparent; }
      .px-field input:focus-visible { outline: none; }
      .px-field input::-webkit-inner-spin-button, .px-field input::-webkit-outer-spin-button { appearance: none; margin: 0; }
      .px-field .unit { font-size: 11px; color: var(--ga-text-muted); }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .bar select { min-height: 26px; }
      .spacer { flex: 1; }
      .notes { display: grid; gap: 2px; margin: 0; padding: 0; list-style: none; font-size: 11px; color: var(--ga-text-muted); }
      .last-sent { font-size: 11px; }
      .last-sent code { font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      .strips {
        display: flex;
        flex: 1;
        gap: 2px;
        min-height: 0;
        /* No right padding: the sticky masters sit flush with the edge, so no strip shows past them.
           The top padding holds group bands, reserved for every channel so faders stay level. */
        padding: 28px 0 4px 4px;
        overflow-x: auto;
        overflow-y: hidden;
        border-radius: 3px;
        background: var(--ga-surface-inset);
      }
      /* Auto: channels share the row between the limits, and scroll once they reach the floor. Fixed: every channel is --strip-width. */
      .strips ga-channel { flex: 1 1 0; min-width: var(--strip-width-min); max-width: var(--strip-width-max); }
      .strips.fixed ga-channel { flex: 0 0 var(--strip-width); min-width: 0; max-width: none; }
      /* A group grows like its channels together: n channels' share, limits and gaps. */
      ga-channel-group {
        flex: var(--members) var(--members) 0;
        min-width: calc(var(--members) * var(--strip-width-min) + (var(--members) - 1) * 2px);
        max-width: calc(var(--members) * var(--strip-width-max) + (var(--members) - 1) * 2px);
      }
      .strips.fixed ga-channel-group { flex: 0 0 auto; min-width: 0; max-width: none; }
      ga-channel-group[collapsed] { flex: 0 0 28px; min-width: 28px; max-width: 28px; }
      .add {
        flex: 0 0 36px;
        min-height: 0;
        border: 1px dashed var(--ga-border-strong);
        background: transparent;
        color: var(--ga-text-secondary);
        font-size: 20px;
      }
      .add:hover:not(:disabled) { color: var(--ga-text-primary); }
      .masters {
        position: sticky;
        right: 0;
        z-index: 1;
        display: flex;
        gap: 2px;
        margin: -28px 0 -4px auto;
        padding: 28px 4px 4px 6px;
        background: var(--ga-surface-inset);
        box-shadow: -8px 0 8px -4px rgb(0 0 0 / 0.5);
      }
      .masters:empty { display: none; }
      .masters ga-strip { flex: 0 0 78px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const topology = store.topology(deviceId);
    if (topology === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so it has no mixer.`));
      return;
    }
    const channels = store.channels(deviceId);

    const devices = h("select", {
      "aria-label": "Device",
      "on:change": (event) => {
        location.hash = href({ page: "mixer", id: (event.target as HTMLSelectElement).value });
      },
    });
    const metered = h("select", { "aria-label": "Metered mix", "data-testid": "metered-mix", "on:change": () => (channels.meteredMix.value = Number(metered.value)) });
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent" });
    const mixer0 = store.mixer(deviceId, 0);
    const notes = h(
      "ul",
      { class: "notes" },
      h("li", {}, "A channel works once it has an input and a main mix. Its fader sets its level in the main mix; sends set its level in other mixes."),
      mixer0.stateKnown ? [] : [h("li", {}, "The device's current mixer levels cannot be read yet, so controls start at defaults and send when changed.")],
      mixer0.hasSend ? [h("li", {}, "The strip's Send shows the raw value: its scale has not been verified.")] : [],
    );

    const strips = h("div", { class: "strips" });
    const add = h("button", { type: "button", class: "add", title: "Add a channel", "aria-label": "Add a channel", "data-testid": "add-channel", "on:click": () => channels.add() }, "+");
    const masters = h("div", { class: "masters", "aria-label": "Mix masters" });

    const stripWidth = h("input", { type: "number", min: STRIP_WIDTH_MIN, max: STRIP_WIDTH_MAX, step: 1, "aria-label": "Channel width in px", "data-testid": "strip-width" });
    const autoWidth = h("button", { type: "button", title: "Fit channels to the window", "data-testid": "strip-width-auto", "on:click": () => store.setMixerWidth({ auto: true }) }, "Auto");
    const fixedWidth = h(
      "button",
      {
        type: "button",
        title: "Set a channel width; channels scroll when they do not fit",
        "data-testid": "strip-width-fixed",
        "on:click": () => {
          store.setMixerWidth({ auto: false });
          stripWidth.focus();
          stripWidth.select();
        },
      },
      "Fixed",
    );
    const commitWidth = () => {
      const px = Number(stripWidth.value);
      if (stripWidth.value.trim() !== "" && Number.isFinite(px)) store.setMixerWidth({ px });
      stripWidth.value = String(store.mixerWidth.peek().px);
    };
    stripWidth.addEventListener("change", commitWidth);
    stripWidth.addEventListener("keydown", (event) => {
      if (event.key === "Enter") commitWidth();
    });
    const width = h(
      "div",
      { class: "width", role: "group", "aria-label": "Channel width" },
      h("span", { class: "caption", "aria-hidden": "true" }, "Width"),
      h("div", { class: "segmented" }, autoWidth, fixedWidth),
      h("label", { class: "px-field" }, stripWidth, h("span", { class: "unit" }, "px")),
    );
    strips.style.setProperty("--strip-width-min", `${STRIP_WIDTH_MIN}px`);
    strips.style.setProperty("--strip-width-max", `${STRIP_WIDTH_MAX}px`);

    this.root.replaceChildren(
      h("div", { class: "bar" }, devices, h("label", { class: "width" }, h("span", { class: "caption" }, "Meters"), metered), h("span", { class: "spacer" }), width, lastSent),
      notes,
      strips,
    );

    // A device without a layout imports one from its routing, once the workspace has loaded.
    let imported = false;
    this.watch(() => {
      if (store.workspace.value === undefined || imported || channels.configured) return;
      imported = true;
      void channels.importFromDevice();
    });

    // Point the device's meters at the chosen mix.
    this.watch(() => store.mixer(deviceId, channels.meteredMix.value).activate());

    // Channels in layout order (consecutive channels of one group inside a group element), then
    // "+", then the masters of the mixes in use.
    const elements = new Map<string, HTMLElement>();
    const groupElements = new Map<string, HTMLElement>();
    let structure = "";
    let mastersKey = "";
    this.watch(() => {
      const layout = channels.layout.value;
      const list = layout.channels;
      for (const id of [...elements.keys()]) if (!list.some((c) => c.id === id)) elements.delete(id);
      const channelElement = (id: string, slot: number) => {
        let element = elements.get(id);
        if (element === undefined) {
          element = h("ga-channel", { "device-id": deviceId, "channel-id": id, "data-channel-slot": String(slot) });
          elements.set(id, element);
        }
        return element;
      };
      const runs: { group: string | undefined; members: { id: string; slot: number }[] }[] = [];
      for (const c of list) {
        const group = c.group !== undefined && layout.groups.some((g) => g.id === c.group) ? c.group : undefined;
        const last = runs.at(-1);
        if (group !== undefined && last?.group === group) last.members.push({ id: c.id, slot: c.slot });
        else runs.push({ group, members: [{ id: c.id, slot: c.slot }] });
      }
      const runsKey = runs.map((run) => `${run.group ?? "-"}:${run.members.map((m) => m.id).join(",")}`).join("|");
      if (runsKey !== structure) {
        structure = runsKey;
        const seen = new Map<string, number>();
        const kept = new Set<string>();
        const children = runs.map((run) => {
          if (run.group === undefined) return run.members.map((m) => channelElement(m.id, m.slot));
          // A group split by moving a channel on its own gets one element per run.
          const count = seen.get(run.group) ?? 0;
          seen.set(run.group, count + 1);
          const groupKey = `${run.group}#${count}`;
          kept.add(groupKey);
          let element = groupElements.get(groupKey);
          if (element === undefined) {
            element = h("ga-channel-group", { "device-id": deviceId, "group-id": run.group });
            groupElements.set(groupKey, element);
          }
          element.style.setProperty("--members", String(run.members.length));
          element.replaceChildren(...run.members.map((m) => channelElement(m.id, m.slot)));
          return element;
        });
        for (const groupKey of [...groupElements.keys()]) if (!kept.has(groupKey)) groupElements.delete(groupKey);
        strips.replaceChildren(...children.flat(), add, masters);
      }
      add.disabled = list.length >= 32 - channels.firstSlot;

      const used = [...new Set(list.flatMap((c) => (channels.isActive(c) ? [c.main_mix as number, ...c.sends] : [])))].sort((a, b) => a - b);
      const key = used.map((mix) => `${mix}:${channels.mixName(mix)}`).join("|");
      if (key !== mastersKey) {
        mastersKey = key;
        masters.replaceChildren(...used.map((mix) => h("ga-strip", { "device-id": deviceId, mixer: String(mix), strip: "master", label: channels.mixName(mix), "data-mix": String(mix) })));
      }
    });

    this.watch(() => {
      const { auto, px } = store.mixerWidth.value;
      autoWidth.setAttribute("aria-pressed", String(auto));
      fixedWidth.setAttribute("aria-pressed", String(!auto));
      stripWidth.disabled = auto;
      if (this.root.activeElement !== stripWidth) stripWidth.value = String(px);
      strips.classList.toggle("fixed", !auto);
      strips.style.setProperty("--strip-width", `${px}px`);
    });
    this.watch(() => {
      metered.replaceChildren(...Array.from({ length: channels.mixCount }, (_, mix) => h("option", { value: String(mix) }, channels.mixName(mix))));
      metered.value = String(channels.meteredMix.value);
    });
    this.watch(() => {
      const known = store.devices.value.filter((d) => d.family !== null);
      devices.replaceChildren(...known.map((d) => h("option", { value: d.id, selected: d.id === deviceId }, d.model ?? d.id)));
      devices.value = deviceId;
      devices.disabled = !store.connected.value;
      add.toggleAttribute("data-offline", !store.connected.value);
    });
    this.watch(() => {
      // Meter gradient stops are dBFS; place them on the same scale the meters use.
      this.style.setProperty("--mixer-meter-gradient", meterGradient(store.theme.value.meter.gradient, undefined, "to top", (db) => meterDeflection(-db)));
    });
    this.watch(() => {
      const sent = store.lastSent.value;
      const dryRun = store.server.value.dry_run;
      if (sent === undefined || sent.deviceId !== deviceId) {
        lastSent.textContent = dryRun ? "Dry run: nothing is written to the device" : "";
        return;
      }
      lastSent.replaceChildren(`${sent.dryRun ? "Dry run, would send" : "Sent"} ${sent.command}: `, h("code", {}, sent.hex));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-mixer": GaMixer;
  }
}
