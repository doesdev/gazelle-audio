// <ga-device-status device-id="…">: one device's identity, its name (editable while connected),
// and a few live values from its status report.

import { h } from "../core/dom.ts";
import { BRIGHTNESS_MAX, displayName } from "../store/store.ts";
import { bindControl } from "./controls.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";

const STATUS_REPORT = "0x73";

/** How long a first Standby click waits for its confirmation, as 48V does. */
const ARM_MS = 3000;

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
      .power { display: flex; gap: 6px; margin-top: 8px; }
      .brightness-row { display: grid; grid-template-columns: minmax(72px, 110px) minmax(0, 1fr); align-items: center; gap: 10px; margin-top: 8px; }
      .brightness-row .caption { font-size: 12px; color: var(--ga-text-secondary); }
      .brightness { position: relative; height: 22px; margin-top: 8px; border: 1px solid var(--ga-border-subtle); border-radius: 3px; background: var(--ga-surface-inset); cursor: ew-resize; touch-action: none; outline: none; }
      .brightness:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .brightness .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
      .brightness .value { position: absolute; inset: 0; font-size: 11px; line-height: 20px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
      .brightness[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .clock select { min-height: 24px; }
      .lock { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-muted); background: var(--ga-surface-inset); }
      .lock[data-locked] { color: var(--ga-text-inverse); background: var(--ga-accent); }
      .note-inline { margin: 6px 0 0; font-size: 11px; color: var(--ga-text-muted); }
      .power button { min-height: 26px; font-size: 12px; font-weight: 600; }
      .standby[data-armed] { outline: 2px dashed var(--ga-state-mute); outline-offset: -2px; }
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

    // Power: both models take `set_power` and report `power_on`. Standby stops the device's audio,
    // so it takes a confirming second click, as 48V does. A device of unknown model gets neither.
    let powerControls: HTMLElement | undefined;
    if (device.family !== null) {
      const powerOn = h("button", { type: "button", "data-testid": "device-power-on", "on:click": () => store.setPower(id, true) }, "Power on");
      let armTimer: ReturnType<typeof setTimeout> | undefined;
      const disarm = () => {
        clearTimeout(armTimer);
        armTimer = undefined;
        standby.removeAttribute("data-armed");
        standby.textContent = "Standby";
      };
      const standby = h(
        "button",
        {
          type: "button",
          class: "standby",
          "data-testid": "device-standby",
          title: "Put the device in standby: click twice",
          "on:click": () => {
            if (armTimer !== undefined) {
              disarm();
              store.setPower(id, false);
              return;
            }
            standby.setAttribute("data-armed", "");
            standby.textContent = "Confirm standby";
            armTimer = setTimeout(disarm, ARM_MS);
          },
        },
        "Standby",
      );
      this.onDisconnect(disarm);
      powerControls = h("div", { class: "power", role: "group", "aria-label": "Device power" }, powerOn, standby);

      // Front-panel brightness, 0..100 as both panels' sliders use.
      const fill = h("div", { class: "fill" });
      const shown = h("span", { class: "value" });
      const brightness = h("div", { class: "brightness", role: "slider", tabindex: 0, "aria-label": "Front-panel brightness", "aria-valuemin": 0, "aria-valuemax": BRIGHTNESS_MAX, "data-testid": "device-brightness" }, fill, shown);
      const reported = () => Number(store.field(id, STATUS_REPORT, "brightness").peek() ?? 0);
      bindControl(brightness, { axis: "x", min: 0, max: BRIGHTNESS_MAX, up: 1, page: 10, reset: 50, get: reported, set: (v) => store.setBrightness(id, v), enabled: () => store.connected.peek() });
      this.watch(() => {
        const value = Math.min(BRIGHTNESS_MAX, Math.max(0, Number(store.field(id, STATUS_REPORT, "brightness").value ?? 0)));
        fill.style.width = `${value}%`;
        shown.textContent = `${value}%`;
        brightness.setAttribute("aria-valuenow", String(value));
        brightness.setAttribute("aria-valuetext", `${value}%`);
        brightness.setAttribute("aria-disabled", String(!store.connected.value));
      });
      powerControls = h("div", {}, powerControls, h("div", { class: "brightness-row" }, h("span", { class: "caption" }, "Brightness"), brightness));
      this.watch(() => {
        const connected = store.connected.value;
        powerOn.disabled = !connected;
        standby.disabled = !connected;
        if (!connected) disarm();
      });
    }

    // Clock: the source and sample rate the device runs at, and what it measures.
    let clockSection: HTMLElement | undefined;
    const clock = store.clock(id);
    if (clock !== undefined) {
      const source = h(
        "select",
        { "aria-label": "Clock source", "data-testid": "clock-source", "on:change": () => store.setClockSource(id, Number(source.value)) },
        clock.sources.map((name, index) => h("option", { value: String(index) }, name)),
      );
      const rate = h(
        "select",
        { "aria-label": "Sample rate", "data-testid": "clock-rate", "on:change": () => store.setSampleRate(id, Number(rate.value)) },
        clock.rates.map((name, index) => h("option", { value: String(index) }, name)),
      );
      const lock = h("span", { class: "lock" }, "NO LOCK");
      const measured = h("span", { class: "readout", "data-testid": "clock-measured" }, "—");
      clockSection = h(
        "ga-section",
        { heading: "Clock" },
        h(
          "dl",
          { class: "fields clock" },
          field("Source", source),
          field("Sample rate", rate),
          field("Measured", h("span", {}, measured, " ", lock)),
        ),
        h("p", { class: "note-inline" }, "While the device follows an external clock, it takes the rate from that source and ignores the sample rate here."),
      );
      this.watch(() => {
        const state = store.clockState(id);
        const connected = store.connected.value;
        source.disabled = !connected;
        rate.disabled = !connected;
        if (state === undefined) return;
        if (this.root.activeElement !== source) source.value = String(Math.min(clock.sources.length - 1, Math.max(0, state.source)));
        if (this.root.activeElement !== rate) rate.value = String(state.rate);
        measured.textContent = state.hz > 0 ? `${(state.hz / 1000).toFixed(1)} kHz` : "—";
        lock.textContent = state.locked ? "LOCKED" : "NO LOCK";
        lock.toggleAttribute("data-locked", state.locked);
      });
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
      ...(clockSection === undefined ? [] : [clockSection]),
      ...(powerControls === undefined ? [] : [powerControls]),
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
