// <ga-mixer device-id="…" mixer="0">: one device's mixer. Tabs pick the mixer (named after the
// output each one feeds), the strips scroll horizontally with the master on the right, and the
// page says plainly what the web mixer cannot know yet. In dry run it shows the bytes of the last
// command sent.

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
      /* Channel width: a caption, an Auto | Fixed segmented control and an inset px field, all the Mix tabs' height. */
      .width { display: flex; align-items: center; gap: 6px; }
      .width .caption { font-size: 11px; color: var(--ga-text-secondary); }
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
      .tabs { display: flex; gap: 2px; }
      .tabs a {
        padding: 4px 10px;
        border-radius: 3px;
        background: var(--ga-control-background);
        color: var(--ga-text-secondary);
        font-family: "Josefin Sans Variable", system-ui, sans-serif;
        font-size: 13px;
        font-weight: 600;
      }
      .tabs a:hover { background: var(--ga-control-hover); color: var(--ga-text-primary); }
      .tabs a[aria-current="page"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .spacer { flex: 1; }
      .notes { display: grid; gap: 2px; margin: 0; padding: 0; list-style: none; font-size: 11px; color: var(--ga-text-muted); }
      .last-sent { font-size: 11px; }
      .last-sent code { font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      .strips {
        display: flex;
        flex: 1;
        gap: 2px;
        min-height: 0;
        /* No right padding: the sticky master sits flush with the edge, so no strip shows past it. */
        padding: 4px 0 4px 4px;
        overflow-x: auto;
        overflow-y: hidden;
        border-radius: 3px;
        background: var(--ga-surface-inset);
      }
      /* Auto: strips share the row between the limits, and scroll once they reach the floor. Fixed: every strip is --strip-width. */
      .strips ga-strip { flex: 1 1 0; min-width: var(--strip-width-min); max-width: var(--strip-width-max); }
      .strips.fixed ga-strip { flex: 0 0 var(--strip-width); min-width: 0; max-width: none; }
      .strips .master ga-strip { flex: 0 0 78px; min-width: 0; max-width: none; }
      .strips .master { margin-left: auto; }
      .master {
        position: sticky;
        right: 0;
        z-index: 1;
        display: flex;
        margin: -4px 0;
        padding: 4px 4px 4px 6px;
        background: var(--ga-surface-inset);
        box-shadow: -8px 0 8px -4px rgb(0 0 0 / 0.5);
      }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const index = Number(this.getAttribute("mixer") ?? "0");
    const topology = store.topology(deviceId);
    if (topology === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so it has no mixer.`));
      return;
    }
    const mixer = store.mixer(deviceId, Math.min(Math.max(0, Number.isInteger(index) ? index : 0), topology.mixers.count - 1));
    this.onDisconnect(mixer.activate());

    const outputName = (groupId: string | undefined) => topology.inputs.find((g) => g.id === groupId)?.name;
    const tabs = h(
      "nav",
      { class: "tabs", "aria-label": "Mixers" },
      Array.from({ length: topology.mixers.count }, (_, i) =>
        h("a", { href: href({ page: "mixer", id: deviceId, sub: String(i) }), "aria-current": i === mixer.index ? "page" : undefined, title: outputName(topology.mixers.outputGroups[i]) }, `Mix ${i + 1}`),
      ),
    );
    const devices = h("select", {
      "aria-label": "Device",
      "on:change": (event) => {
        location.hash = href({ page: "mixer", id: (event.target as HTMLSelectElement).value, sub: "0" });
      },
    });
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent" });
    const notes = h("ul", { class: "notes" });
    const noteItems = [`Feeds ${outputName(topology.mixers.outputGroups[mixer.index]) ?? "an unnamed output"}.`];
    if (!mixer.stateKnown) noteItems.push("The device's current mixer settings cannot be read yet, so controls start at defaults and send when changed.");
    if (!mixer.meterSourceSelectable) noteItems.push("This mixer's meters cannot be selected on this model; they show the source the device last used.");
    if (mixer.hasSend) noteItems.push("Send shows the raw value: its scale has not been verified.");
    notes.replaceChildren(...noteItems.map((text) => h("li", {}, text)));

    const strips = h("div", { class: "strips" }, Array.from({ length: mixer.channels }, (_, i) => h("ga-strip", { "device-id": deviceId, mixer: String(mixer.index), strip: String(i) })), h("div", { class: "master" }, h("ga-strip", { "device-id": deviceId, mixer: String(mixer.index), strip: "master" })));

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

    this.root.replaceChildren(h("div", { class: "bar" }, devices, tabs, h("span", { class: "spacer" }), width, lastSent), notes, strips);

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
      const known = store.devices.value.filter((d) => d.family !== null);
      devices.replaceChildren(...known.map((d) => h("option", { value: d.id, selected: d.id === deviceId }, d.model ?? d.id)));
      devices.value = deviceId;
      devices.disabled = !store.connected.value;
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
