// <ga-device-status device-id="…">: one device's identity, its name (editable while connected),
// and a few live values from its status report.

import { h } from "../core/dom.ts";
import { BRIGHTNESS_MAX, displayName, OSCILLATOR_FREQUENCIES, OSCILLATOR_LEVELS, PRESET_SLOTS, type OscillatorState } from "../store/store.ts";
import { bindConfirm, bindControl, confirmedChoice } from "./controls.ts";
import { driverSection } from "./driver-section.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";
import type { GaSection } from "./section.ts";
import { keepCollapsed } from "./view-state.ts";

const STATUS_REPORT = "0x73";

/** How long a first Standby click waits for its confirmation, as 48V does. */
const ARM_MS = 3000;

const LIVE_FIELDS: readonly [field: string, label: string, format: (value: unknown) => string, family?: "quadro" | "studio"][] = [
  ["power_on", "Power", (value) => (value ? "On" : "Standby")],
  // The slot the device is on: only where a preset can be recalled, so not on the Quadro.
  ["current_preset", "Preset", (value) => String(value), "studio"],
  ["sync_source", "Sync source", (value) => String(value)],
];

const hex4 = (n: number) => n.toString(16).padStart(4, "0");

/** Each live field's key for the explain mode. */
const LIVE_KEYS: Record<string, string> = { power_on: "devices.live-power", current_preset: "devices.live-preset", sync_source: "devices.live-sync" };

export class GaDeviceStatus extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; max-width: 640px; }
      ga-section + ga-section { margin-top: 10px; }
      .power { display: flex; gap: 6px; margin-top: 8px; }
      .brightness-row { display: grid; grid-template-columns: minmax(72px, 110px) minmax(0, 1fr); align-items: center; gap: 10px; margin-top: 8px; }
      .brightness-row .caption { font-size: 12px; color: var(--ga-text-secondary); }
      .brightness { position: relative; height: 22px; margin-top: 8px; border: 1px solid var(--ga-border-subtle); border-radius: 3px; background: var(--ga-surface-inset); cursor: ew-resize; touch-action: none; outline: none; }
      .brightness:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .brightness .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
      .brightness .value { position: absolute; inset: 0; font-size: 11px; line-height: 20px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
      .brightness[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .choice { display: inline-flex; align-items: center; gap: 6px; }
      .lock { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-muted); background: var(--ga-surface-inset); }
      .lock[data-locked] { color: var(--ga-text-inverse); background: var(--ga-accent); }
      .note-inline { margin: 6px 0 0; font-size: 11px; color: var(--ga-text-muted); }
      .presets { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
      .presets button { min-width: 34px; min-height: 26px; font-size: 12px; font-weight: 600; }
      .presets button[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .presets .save[data-armed] { outline: 2px dashed var(--ga-state-mute); outline-offset: -2px; }
      .presets .spacer { flex: 1; }
      .tone-row { display: flex; align-items: center; gap: 6px; }
      .tone[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
      .power button { min-height: 26px; font-size: 12px; font-weight: 600; }
      .standby[data-armed] { outline: 2px dashed var(--ga-state-mute); outline-offset: -2px; }
      .presets button[data-armed], .tone[data-armed], .dc[data-armed], .confirm { outline: 2px dashed var(--ga-state-mute); outline-offset: -2px; }
      .driver-safe, .driver-force { min-width: 44px; min-height: 24px; font-size: 12px; font-weight: 600; }
      .driver-safe[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      /* The converter shows whether it is on in the accent, as a surface's SRC button does: the
         faint lift a pressed button gets by default read as no indicator at all (the user). */
      .spdif-src[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .driver-safe[data-armed], .driver-force[data-armed] { outline: 2px dashed var(--ga-state-mute); outline-offset: -2px; }
      .note-inline.warning { color: var(--ga-state-mute); }
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
      "data-explain": "devices.name",
    });
    const showName = commitOnEnter(name, (value) => store.renameDevice(id, value), () => store.workspace.peek()?.aliases[id] ?? "", store.view<string | undefined>(`draft:devices:${id}:name`, undefined));
    const field = (label: string, value: Node | string) => [h("dt", {}, label), h("dd", {}, value)];

    const live = h("dl", { class: "fields" });
    const liveSection = h("ga-section", { heading: "Status report", explain: "devices.status" }, live);
    if (device.family === null) {
      live.replaceChildren(h("dd", { class: "muted" }, "This device's model is unknown, so its reports cannot be decoded."));
    } else {
      this.onDisconnect(store.watchReport(id, STATUS_REPORT));
      for (const [fieldName, label, format, only] of LIVE_FIELDS) {
        if (only !== undefined && only !== device.family) continue;
        const readout = h("span", { class: "readout", "data-field": fieldName, "data-explain": LIVE_KEYS[fieldName] }, "…");
        live.append(...field(label, readout));
        this.watch(() => {
          const value = store.field(id, STATUS_REPORT, fieldName).value;
          readout.textContent = value === undefined ? "…" : format(value);
        });
      }
    }

    // Power: both models take `set_power` and report `power_on`. Standby stops the device's audio,
    // so it takes a confirming second click, as 48V does. A device of unknown model gets neither.
    let powerControls: HTMLElement | undefined;
    if (device.family !== null) {
      const powerOn = h("button", { type: "button", "data-testid": "device-power-on", "data-explain": "devices.power-on", "on:click": () => store.setPower(id, true) }, "Power on");
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
          "data-explain": "devices.standby",
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
      const brightness = h("div", { class: "brightness", role: "slider", tabindex: 0, "aria-label": "Front-panel brightness", "aria-valuemin": 0, "aria-valuemax": BRIGHTNESS_MAX, "data-testid": "device-brightness", "data-explain": "devices.brightness" }, fill, shown);
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

    // Presets: the device's own five slots, offered on the Studio+ only. The Quadro accepts a
    // recall and does nothing with it (measured at the device, 2026-09-20), and Antelope's own
    // Quadro panel never sends one: what it calls presets there are its session files. Saving into
    // a slot nothing can recall would be a trap, so the Quadro is offered neither.
    // Recall may change anything at once, 48V and the clock among it, and saving overwrites the
    // slot, so each takes a confirming second click, as 48V does.
    let presetSection: HTMLElement | undefined;
    if (device.family === "studio") {
      const slots = Array.from({ length: PRESET_SLOTS }, (_, i) => i + 1);
      const buttons = slots.map((slot) =>
        h("button", { type: "button", "data-testid": `preset-${slot}`, "aria-label": `Recall preset ${slot}`, title: `Recall preset ${slot}: click twice`, "data-explain": "devices.preset-recall", "data-explain-name": String(slot) }, String(slot)),
      );
      const recallDisarms = buttons.map((button, i) => bindConfirm(button, String(slots[i]), () => store.recallPreset(id, slots[i] as number)));
      for (const disarmRecall of recallDisarms) this.onDisconnect(disarmRecall);
      const into = h("select", { "aria-label": "Preset to save into", "data-testid": "preset-save-slot", "data-explain": "devices.preset-slot" }, slots.map((slot) => h("option", { value: String(slot) }, String(slot))));
      let armTimer: ReturnType<typeof setTimeout> | undefined;
      const disarm = () => {
        clearTimeout(armTimer);
        armTimer = undefined;
        save.removeAttribute("data-armed");
        save.textContent = "Save";
      };
      const save = h(
        "button",
        {
          type: "button",
          class: "save",
          "data-testid": "preset-save",
          "data-explain": "devices.preset-save",
          title: "Save the device's current state into the chosen preset: click twice",
          "on:click": () => {
            if (armTimer !== undefined) {
              disarm();
              store.savePreset(id, Number(into.value));
              return;
            }
            save.setAttribute("data-armed", "");
            save.textContent = "Confirm save";
            armTimer = setTimeout(disarm, ARM_MS);
          },
        },
        "Save",
      );
      this.onDisconnect(disarm);
      presetSection = h(
        "ga-section",
        { heading: "Presets", explain: "devices.presets" },
        h("div", { class: "presets" }, buttons, h("span", { class: "spacer" }), h("span", { class: "caption" }, "Save into"), into, save),
        h("p", { class: "note-inline" }, "The device's own presets, not the workspace layout. Saving overwrites what is in that slot."),
      );
      this.watch(() => {
        const current = Number(store.field(id, STATUS_REPORT, "current_preset").value ?? 0);
        const connected = store.connected.value;
        for (const [i, button] of buttons.entries()) {
          button.setAttribute("aria-pressed", String(current === slots[i]));
          button.disabled = !connected;
        }
        into.disabled = !connected;
        save.disabled = !connected;
        if (!connected) {
          disarm();
          for (const disarmRecall of recallDisarms) disarmRecall();
        }
      });
    }

    // Clock: the source and sample rate the device runs at, and what it measures. Neither takes the
    // wheel: each step would reclock the device, interrupting everything playing through it,
    // and a source with no signal behind it loses lock. For the same reason a choice is only sent
    // from a Confirm button beside the menu, which a wait takes away again, putting the menu back.
    let clockSection: HTMLElement | undefined;
    const clock = store.clock(id);
    if (clock !== undefined) {
      const source = h(
        "select",
        { "aria-label": "Clock source", "data-testid": "clock-source", "data-no-wheel": true, "data-explain": "devices.clock-source" },
        clock.sources.map((name, index) => h("option", { value: String(index) }, name)),
      );
      const rate = h(
        "select",
        { "aria-label": "Sample rate", "data-testid": "clock-rate", "data-no-wheel": true, "data-explain": "devices.sample-rate" },
        clock.rates.map((name, index) => h("option", { value: String(index) }, name)),
      );
      const sourceChoice = confirmedChoice(source, "clock-source-confirm", "devices.clock-confirm", (index) => `Change the clock source to ${clock.sources[index] ?? index}: audio stops for a moment`, (index) => store.setClockSource(id, index));
      const rateChoice = confirmedChoice(rate, "clock-rate-confirm", "devices.clock-confirm", (index) => `Change the sample rate to ${clock.rates[index] ?? index}: audio stops for a moment`, (index) => store.setSampleRate(id, index));
      this.onDisconnect(sourceChoice.disarm);
      this.onDisconnect(rateChoice.disarm);
      const lock = h("span", { class: "lock", "data-explain": "devices.lock" }, "NO LOCK");
      const measured = h("span", { class: "readout", "data-testid": "clock-measured", "data-explain": "devices.measured" }, "…");
      // The Studio+'s S/PDIF sample-rate converter: with it on, a digital input at another rate or on
      // another clock is converted rather than having to be the clock. A switch, as the panel's is.
      const spdifSrc = store.hasSpdifSrc(id)
        ? h("button", {
            type: "button",
            class: "spdif-src",
            "data-control": "",
            "data-testid": "spdif-src",
            "data-explain": "devices.spdif-src",
            "aria-label": "S/PDIF sample-rate converter",
            title: "Convert the S/PDIF input's sample rate, so it need not follow the device's clock",
            "on:click": () => store.setSpdifSrc(id, !(store.spdifSrc(id) ?? false)),
          }, "Converter")
        : undefined;
      clockSection = h(
        "ga-section",
        { heading: "Clock", explain: "devices.clock" },
        h(
          "dl",
          { class: "fields clock" },
          field("Source", h("span", { class: "choice" }, source, sourceChoice.confirm)),
          field("Sample rate", h("span", { class: "choice" }, rate, rateChoice.confirm)),
          field("Measured", h("span", {}, measured, " ", lock)),
          ...(spdifSrc === undefined ? [] : field("S/PDIF SRC", spdifSrc)),
        ),
        h("p", { class: "note-inline" }, "While the device follows an external clock, it takes the rate from that source and ignores the sample rate here."),
      );
      this.watch(() => {
        const state = store.clockState(id);
        const connected = store.connected.value;
        source.disabled = !connected;
        rate.disabled = !connected;
        if (!connected) {
          sourceChoice.disarm();
          rateChoice.disarm();
        }
        if (spdifSrc !== undefined) {
          spdifSrc.setAttribute("aria-pressed", String(store.spdifSrc(id) ?? false));
          (spdifSrc as HTMLButtonElement).disabled = !connected;
        }
        if (state === undefined) return;
        sourceChoice.current = Math.min(clock.sources.length - 1, Math.max(0, state.source));
        rateChoice.current = state.rate;
        if (this.root.activeElement !== source && !sourceChoice.armed()) source.value = String(sourceChoice.current);
        if (this.root.activeElement !== rate && !rateChoice.armed()) rate.value = String(rateChoice.current);
        measured.textContent = state.hz > 0 ? `${(state.hz / 1000).toFixed(1)} kHz` : "none";
        lock.textContent = state.locked ? "LOCKED" : "NO LOCK";
        lock.toggleAttribute("data-locked", state.locked);
      });
    }

    // Test oscillator: a tone per side over a shared level, for lining up a signal path. Both
    // models have it. The device holds the two as mute bits; here they are tones you switch on,
    // which is how they are used, and a tone at 0 dBFS is loud, so the note says so and turning one
    // on takes a confirming second click, as 48V does. Off is one click.
    let oscSection: HTMLElement | undefined;
    if (device.family !== null) {
      const state = () => store.oscillator(id) as OscillatorState;
      const freq = (side: "left" | "right") => {
        const select = h(
          "select",
          { "aria-label": `Oscillator ${side} frequency`, "data-testid": `osc-freq-${side}`, "data-explain": "devices.osc-frequency", "data-explain-name": side === "left" ? "Left" : "Right", "on:change": () => store.setOscillator(id, { [side]: Number(select.value) }) },
          OSCILLATOR_FREQUENCIES.map((name, index) => h("option", { value: String(index) }, name)),
        );
        return select;
      };
      const on = (side: "left" | "right") => {
        const button = h("button", {
          type: "button",
          class: "tone",
          "data-control": "",
          "data-testid": `osc-on-${side}`,
          "data-explain": "devices.osc-tone",
          "data-explain-name": side === "left" ? "Left" : "Right",
          "aria-label": `Oscillator ${side} on`,
          title: "A tone straight to the outputs: click twice to turn it on",
        }, "Tone");
        const isOn = () => (side === "left" ? state().onLeft : state().onRight);
        const disarmTone = bindConfirm(button, "Tone", () => store.setOscillator(id, side === "left" ? { onLeft: !isOn() } : { onRight: !isOn() }), () => !isOn());
        this.onDisconnect(disarmTone);
        toneDisarms.push(disarmTone);
        return button;
      };
      const toneDisarms: (() => void)[] = [];
      const level = h(
        "select",
        { "aria-label": "Oscillator level", "data-testid": "osc-level", "data-explain": "devices.osc-level", "on:change": () => store.setOscillator(id, { level: Number(level.value) }) },
        OSCILLATOR_LEVELS.map((name, index) => h("option", { value: String(index) }, name)),
      );
      const sides = [
        { side: "left" as const, name: "Left", freq: freq("left"), on: on("left") },
        { side: "right" as const, name: "Right", freq: freq("right"), on: on("right") },
      ];
      oscSection = h(
        "ga-section",
        { heading: "Test oscillator", explain: "devices.oscillator" },
        h(
          "dl",
          { class: "fields osc" },
          ...sides.map(({ name, freq: select, on: button }) => field(name, h("span", { class: "tone-row" }, select, button))),
          field("Level", level),
        ),
        h("p", { class: "note-inline" }, "A sine tone straight to the outputs, for lining up a signal path. At 0 dBFS it is as loud as the device goes."),
      );
      this.watch(() => {
        const current = state();
        const connected = store.connected.value;
        level.disabled = !connected;
        if (!connected) for (const disarmTone of toneDisarms) disarmTone();
        if (this.root.activeElement !== level) level.value = String(current.level);
        for (const { side, freq: select, on: button } of sides) {
          select.disabled = !connected;
          if (this.root.activeElement !== select) select.value = String(current[side]);
          button.setAttribute("aria-pressed", String(side === "left" ? current.onLeft : current.onRight));
          (button as HTMLButtonElement).disabled = !connected;
        }
      });
    }

    // DC coupling: whether the converters pass DC, for control voltages rather than audio. One
    // switch per side, as the Quadro's settings page has, and each is reported back. DC can harm
    // speakers, so turning a side on takes a confirming second click, as 48V does; off is one.
    let dcSection: HTMLElement | undefined;
    if (store.hasDcCoupling(id)) {
      const sides = [
        { side: "inputs" as const, name: "Inputs" },
        { side: "outputs" as const, name: "Outputs" },
      ];
      const coupled = (side: "inputs" | "outputs") => store.dcCoupling(id)?.[side] ?? false;
      const switches = sides.map(({ side, name }) =>
        h("button", {
          type: "button",
          class: "dc",
          "data-control": "",
          "data-testid": `dc-${side}`,
          "data-explain": side === "inputs" ? "devices.dc-inputs" : "devices.dc-outputs",
          "aria-label": `DC coupled ${name.toLowerCase()}`,
          title: "Pass DC: click twice to turn it on",
        }, "DC coupled"),
      );
      const dcDisarms = sides.map(({ side }, i) => bindConfirm(switches[i] as HTMLElement, "DC coupled", () => store.setDcCoupled(id, side, !coupled(side)), () => !coupled(side)));
      for (const disarmDc of dcDisarms) this.onDisconnect(disarmDc);
      dcSection = h(
        "ga-section",
        { heading: "DC coupling", explain: "devices.dc" },
        h("dl", { class: "fields" }, ...sides.map(({ name }, i) => field(name, switches[i] as HTMLElement))),
        h("p", { class: "note-inline" }, "Lets the converters pass control voltages as well as audio, for modular gear. Leave it off for audio."),
      );
      this.watch(() => {
        const state = store.dcCoupling(id);
        const connected = store.connected.value;
        sides.forEach(({ side }, i) => {
          const button = switches[i] as HTMLButtonElement;
          button.setAttribute("aria-pressed", String(state?.[side] ?? false));
          button.disabled = !connected;
          if (!connected) dcDisarms[i]?.();
        });
      });
    }

    // Panning law: how much a centre-panned signal is attenuated. Quadro only, and not in the
    // status report, so it is read once here; every mixer pan, and the mono downmix, is heard
    // through it.
    let panningSection: HTMLElement | undefined;
    const panningLaws = store.panningLaws(id);
    if (panningLaws !== undefined) {
      const law = h(
        "select",
        { "aria-label": "Panning law", "data-testid": "panning-law", "data-explain": "devices.panning-law", "on:change": () => store.setPanningLaw(id, Number(law.value)) },
        panningLaws.map((name, index) => h("option", { value: String(index) }, name)),
      );
      panningSection = h(
        "ga-section",
        { heading: "Panning law", explain: "devices.panning" },
        h("dl", { class: "fields" }, field("Centre attenuation", law)),
        h("p", { class: "note-inline" }, "How much a centred signal is attenuated in every mix, including a mix summed to mono."),
      );
      void store.loadPanningLaw(id);
      this.watch(() => {
        law.disabled = !store.connected.value;
        const current = String(store.panningLaw(id).value);
        if (this.root.activeElement !== law) law.value = current;
      });
    }

    this.root.replaceChildren(
      h(
        "ga-section",
        { heading: "Device", explain: "devices.device" },
        h(
          "dl",
          { class: "fields" },
          field("Name", name),
          field("Model", h("span", { class: "readout", "data-explain": "devices.model" }, device.model ?? "Unknown")),
          field("Family", h("span", { class: "readout", "data-explain": "devices.family" }, device.family ?? "unknown")),
          field("Id", h("span", { class: "readout", "data-explain": "devices.id" }, device.id)),
          field("USB id", h("span", { class: "readout", "data-explain": "devices.usb-id" }, `${hex4(device.vid)}:${hex4(device.pid)}`)),
          field("Backend", h("span", { class: "readout", "data-explain": "devices.backend" }, device.backend)),
          field("Identity", h("span", { class: "readout", "data-explain": "devices.identity" }, device.identity_stable ? "Stable" : "Changes on reconnect")),
        ),
      ),
      liveSection,
      ...(clockSection === undefined ? [] : [clockSection]),
      driverSection(store, id, (fn) => this.watch(fn), (fn) => this.onDisconnect(fn)),
      ...(panningSection === undefined ? [] : [panningSection]),
      ...(dcSection === undefined ? [] : [dcSection]),
      ...(oscSection === undefined ? [] : [oscSection]),
      ...(presetSection === undefined ? [] : [presetSection]),
      ...(powerControls === undefined ? [] : [powerControls]),
    );
    // A section closed on one device's page stays closed on every device's, for the tab.
    for (const section of this.root.querySelectorAll<GaSection>("ga-section")) keepCollapsed(section, store.view(`devices:collapsed:${section.getAttribute("heading") ?? ""}`, false));

    this.watch(() => {
      const workspace = store.workspace.value;
      name.disabled = !store.connected.value;
      showName(workspace?.aliases[id] ?? "");
      name.title = `Shown as “${displayName(device, workspace)}”. Leave empty to use the model name.`;
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-device-status": GaDeviceStatus;
  }
}
