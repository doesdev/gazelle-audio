// <ga-device-status device-id="…">: one device's identity, its name (editable while connected),
// and a few live values from its status report.

import { h } from "../core/dom.ts";
import { displayName } from "../store/store.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";

const STATUS_REPORT = "0x73";

const LIVE_FIELDS: readonly [field: string, label: string, format: (value: unknown) => string][] = [
  ["power_on", "Power", (value) => (value ? "On" : "Standby")],
  ["current_preset", "Preset", (value) => String(value)],
  ["sync_source", "Sync source", (value) => String(value)],
];

const hex4 = (n: number) => n.toString(16).padStart(4, "0");

export class GaDeviceStatus extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; max-width: 640px; }
      ga-section + ga-section { margin-top: 10px; }
      .name { width: 100%; max-width: 280px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const id = this.getAttribute("device-id") ?? "";
    const device = store.devices.peek().find((d) => d.id === id);
    if (device === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${id} is not connected.`));
      return;
    }

    const name = h("input", {
      class: "name",
      "aria-label": "Device name",
      placeholder: device.model ?? device.id,
      "data-testid": "device-name",
    });
    commitOnEnter(name, (value) => store.renameDevice(id, value), () => store.workspace.peek()?.aliases[id] ?? "");
    const field = (label: string, value: Node | string) => [h("dt", {}, label), h("dd", {}, value)];

    const live = h("dl", { class: "fields" });
    const liveSection = h("ga-section", { heading: "Status report" }, live);
    if (device.family === null) {
      live.replaceChildren(h("dd", { class: "muted" }, "This device's model is unknown, so its reports cannot be decoded."));
    } else {
      this.onDisconnect(store.watchReport(id, STATUS_REPORT));
      for (const [fieldName, label, format] of LIVE_FIELDS) {
        const readout = h("span", { class: "readout", "data-field": fieldName }, "—");
        live.append(...field(label, readout));
        this.watch(() => {
          const value = store.field(id, STATUS_REPORT, fieldName).value;
          readout.textContent = value === undefined ? "—" : format(value);
        });
      }
    }

    this.root.replaceChildren(
      h(
        "ga-section",
        { heading: "Device" },
        h(
          "dl",
          { class: "fields" },
          field("Name", name),
          field("Model", h("span", { class: "readout" }, device.model ?? "Unknown")),
          field("Family", h("span", { class: "readout" }, device.family ?? "unknown")),
          field("Id", h("span", { class: "readout" }, device.id)),
          field("USB id", h("span", { class: "readout" }, `${hex4(device.vid)}:${hex4(device.pid)}`)),
          field("Backend", h("span", { class: "readout" }, device.backend)),
          field("Identity", h("span", { class: "readout" }, device.identity_stable ? "Stable" : "Changes on reconnect")),
        ),
      ),
      liveSection,
    );

    this.watch(() => {
      const workspace = store.workspace.value;
      name.disabled = !store.connected.value;
      if (this.root.activeElement !== name) name.value = workspace?.aliases[id] ?? "";
      name.title = `Shown as “${displayName(device, workspace)}”. Leave empty to use the model name.`;
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-device-status": GaDeviceStatus;
  }
}
