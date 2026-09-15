// <ga-mixer device-id="…" mixer="0">: one device's mixer. Tabs pick the mixer (named after the
// output each one feeds), the strips scroll horizontally with the master on the right, and the
// page says plainly what the web mixer cannot know yet. In dry run it shows the bytes of the last
// command sent.

import { h } from "../core/dom.ts";
import { meterDeflection } from "../store/mixer.ts";
import { meterGradient } from "../themes/theme.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href } from "./router.ts";

export class GaMixer extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 8px; height: 100%; min-height: 520px; }
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
        padding: 4px;
        overflow-x: auto;
        border-radius: 3px;
        background: var(--ga-surface-inset);
      }
      .master { position: sticky; right: 0; margin-left: 8px; padding-left: 6px; background: var(--ga-surface-inset); }
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

    this.root.replaceChildren(h("div", { class: "bar" }, devices, tabs, h("span", { class: "spacer" }), lastSent), notes, strips);

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
