// <ga-remote device-id="…">: the Remote page, a remote for the session (the user, 2026-09-26): what
// is reached for away from the desk, laid out for a phone held upright, and what the Android shell
// opens (`#/remote`). It works as well in a desktop window, in one centred column.
//
// Top to bottom:
// - **The device**, as every page has it: the one the address names or the one last selected, with a
//   switch between them when more than one is attached.
// - **Monitoring**: the Control Room panel itself (<ga-monitor touch>), so the outputs, Mono,
//   talkback and every rule they keep are the sidebar's own, only larger. On the Quadro, hard mute
//   below it, apart, as the thing to hit when something goes wrong: one tap mutes every output,
//   and letting them all play again takes a second tap.
// - **Mix**: what the mixer dock shows (and the dock hides here): the device's mix, chosen with the
//   dock's own Mix menu and following the Mixer page's Mix buttons, or a surface, chosen with the
//   dock's Show menu. A device's mix is a list of horizontal faders, one per channel and the master
//   last. On a phone upright that gives each fader the screen's width, about 300 px of travel against
//   the dock's 130, room for the whole name, and the page scrolls the way a thumb already moves;
//   vertical faders side by side would scroll sideways under the same thumb that moves them. A
//   surface keeps its own strips, as the dock shows them.
// - **Inputs**: each preamp's gain, type, 48V and phase, the Inputs page's own card. Its type is shown
//   and not changed here: a mistaken tap on Mic would put 48V's button in reach and the gain on another
//   scale, for a change that is made at the desk.
//
// Sections fold, remembered per browser as the sidebar's are. Under a finger no control jumps: the
// touch rules are `bindControl`'s (controls.ts), and a level's reset is a button that asks first.

import { h } from "../core/dom.ts";
import { effect, untracked } from "../core/signal.ts";
import { faderPosition, formatLevel, LEVEL_MAX, levelAtFaderPosition, meterDeflection, type StripId } from "../store/mixer.ts";
import { PREAMP_TYPES } from "../store/inputs.ts";
import { displayName } from "../store/store.ts";
import { meterGradient } from "../themes/theme.ts";
import { channelStrip } from "./channel.ts";
import { bindConfirm, bindControl, levelReset } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { INPUT_CONTROL_STYLES, preampCard, type ControlHost } from "./inputs-page.ts";
import { isReady, loadElement } from "./lazy.ts";
import { animateMeter, METER_FLOOR } from "./meter-motion.ts";
import { followMix } from "./mixer-dock.ts";
import { href } from "./router.ts";
import type { GaSection } from "./section.ts";

/** Where a fader's reset goes: -20 dB, the safe level a double-click gives (the user, 2026-09-18). Never unity here. */
const SAFE_LEVEL = 20;

/** The fader's handle, in pixels; its centre marks the level. */
const HANDLE_PX = 18;

/** The page's sections, in order, by the name each is remembered under. */
type RemoteSection = "monitoring" | "mix" | "inputs";

export class GaRemote extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; }
      .page { display: grid; gap: 10px; max-width: 560px; margin: 0 auto; }
      .placeholder { font-size: 13px; }
      /* The device, and a switch between devices when there is more than one. */
      .devices { display: flex; flex-wrap: wrap; gap: 6px; }
      .devices a {
        display: inline-flex;
        align-items: center;
        min-height: 44px;
        padding: 0 16px;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 4px;
        background: var(--ga-control-background);
        color: var(--ga-control-text);
        font-size: 14px;
        font-weight: 600;
      }
      .devices a[aria-current] { border-color: var(--ga-accent); background: var(--ga-accent); color: var(--ga-accent-text); }
      .device-name { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 16px; font-weight: 600; }
      /* Hard mute: apart from the rest, and the one control on the page in the mute colour. */
      .panic { display: grid; gap: 6px; margin-top: 14px; padding-top: 14px; border-top: 2px solid var(--ga-border-strong, var(--ga-border-subtle)); }
      .panic .note { margin: 0; font-size: 12px; color: var(--ga-text-muted); }
      .hard-mute { min-height: 56px; border: 2px solid var(--ga-state-mute); font-size: 16px; font-weight: 700; letter-spacing: 0.04em; }
      .hard-mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .hard-mute[data-armed] { outline: 2px dashed var(--ga-state-mute); outline-offset: 2px; }
      /* The mix's menus: thumb height, side by side. */
      .choices { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 8px; margin-bottom: 8px; }
      .choices label { display: grid; gap: 2px; font-size: 11px; color: var(--ga-text-secondary); }
      .choices select { width: 100%; min-height: 44px; font-size: 14px; }
      .rows { display: grid; gap: 6px; }
      .rows .empty { margin: 0; padding: 8px 4px; font-size: 13px; color: var(--ga-text-muted); }
      .rows .empty a { color: var(--ga-accent); }
      /* A surface keeps its own strips, in a row that scrolls sideways as the dock's does. */
      .surface { display: flex; gap: 4px; height: 280px; padding: 4px; overflow-x: auto; overflow-y: hidden; border-radius: 4px; background: var(--ga-surface-inset); }
      .surface ga-surface-strip { min-height: 0; }
      /* A preamp card at thumb size. The type is shown, not changed (the page's comment says why). */
      ${INPUT_CONTROL_STYLES}
      .preamps { display: grid; gap: 8px; }
      .preamp { gap: 8px; padding: 10px; }
      .preamp .name { font-size: 16px; }
      .preamp .segmented { display: none; }
      .preamp .type { padding: 2px 8px; border-radius: 3px; background: var(--ga-surface-inset); color: var(--ga-text-secondary); font-size: 12px; font-weight: 700; }
      .preamp .gain { height: 44px; }
      .preamp .gain .value { font-size: 15px; line-height: 42px; }
      .preamp .toggles { gap: 8px; }
      .preamp .toggles button { min-height: 44px; font-size: 14px; }
      .preamp .hpf { padding: 2px 6px; font-size: 11px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    if (store.topology(deviceId) === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so there is nothing to control on it.`));
      return;
    }
    const section = (id: RemoteSection, heading: string, explain: string, ...content: Node[]) => {
      const element = h("ga-section", { heading, explain, touch: "", "data-testid": `remote-section-${id}` }, ...content) as GaSection;
      element.collapsed = store.remoteSectionCollapsed(id);
      element.addEventListener("toggle", () => store.setRemoteSectionCollapsed(id, element.collapsed));
      return element;
    };

    this.root.replaceChildren(
      h(
        "div",
        { class: "page" },
        this.#devices(deviceId),
        section("monitoring", "Monitoring", "remote.monitoring", this.#monitoring(deviceId)),
        section("mix", "Mix", "remote.mix", this.#mix(deviceId)),
        section("inputs", "Inputs", "remote.inputs", this.#inputs(deviceId)),
      ),
    );

    this.watch(() => {
      // Meter gradient stops are dBFS; placed on the scale the meters use, left to right.
      this.style.setProperty("--remote-meter-gradient", meterGradient(store.theme.value.meter.gradient, undefined, "to right", (db) => meterDeflection(-db)));
    });
  }

  /** The device the page acts on, and a switch between devices of known model when there are several. */
  #devices(deviceId: string): HTMLElement {
    const store = useStore();
    const bar = h("nav", { class: "devices", "aria-label": "Device", "data-testid": "remote-devices" });
    this.watch(() => {
      const known = store.devices.value.filter((d) => d.family !== null);
      const workspace = store.workspace.value;
      if (known.length <= 1) {
        const only = known.find((d) => d.id === deviceId);
        bar.replaceChildren(h("span", { class: "device-name" }, only === undefined ? deviceId : displayName(only, workspace)));
        return;
      }
      bar.replaceChildren(
        ...known.map((d) =>
          h("a", { href: href({ page: "remote", id: d.id }), "aria-current": d.id === deviceId ? "page" : undefined, "data-testid": `remote-device-${d.id}`, "data-explain": "remote.device", "data-explain-name": displayName(d, workspace) }, displayName(d, workspace)),
        ),
      );
    });
    return bar;
  }

  /** The Control Room panel at thumb size, rebuilt when its outputs change, and the Quadro's hard mute. */
  #monitoring(deviceId: string): HTMLElement {
    const store = useStore();
    const box = h("div", { class: "monitoring" });
    const panel = h("div", {});
    box.append(panel);
    let shown: string | undefined;
    this.watch(() => {
      // The outputs chosen on the Outputs page; the panel reads them once, so a new choice builds it again.
      const outputs = store.controlRoomOutputs(deviceId).value.join(",");
      if (outputs === shown) return;
      shown = outputs;
      untracked(() => panel.replaceChildren(h("ga-monitor", { "device-id": deviceId, touch: "" })));
    });

    const outputs = store.outputs(deviceId);
    if (!outputs.hasHardMute) return box;
    this.onDisconnect(outputs.activate());
    const hardMute = h("button", { type: "button", class: "hard-mute", "data-testid": "remote-hard-mute", "data-explain": "remote.hard-mute", "aria-label": "Hard mute: mute every output" });
    const idle = () => (outputs.hardMute.peek() ? "Hard mute is on: tap twice to release" : "Hard mute");
    // Muting everything is one tap; letting everything play again is two, as turning 48V on is.
    const disarm = bindConfirm(hardMute, idle, () => outputs.setHardMute(!outputs.hardMute.peek()), () => outputs.hardMute.peek());
    this.onDisconnect(disarm);
    this.watch(() => {
      const on = outputs.hardMute.value;
      hardMute.setAttribute("aria-pressed", String(on));
      hardMute.disabled = !store.connected.value;
      if (!hardMute.hasAttribute("data-armed")) hardMute.textContent = untracked(idle);
    });
    box.append(h("div", { class: "panic" }, hardMute, h("p", { class: "note" }, "Mutes every output of the Quadro at once, whatever their volumes.")));
    return box;
  }

  /** The dock's mix, or its surface, with the dock's two menus. */
  #mix(deviceId: string): HTMLElement {
    const store = useStore();
    const channels = store.channels(deviceId);
    const mixSelect = h("select", { "aria-label": "Mix", "data-testid": "remote-mix-select", "data-explain": "remote.mix-select" });
    const sourceSelect = h("select", { "aria-label": "Show", "data-testid": "remote-source-select", "data-explain": "remote.source", "on:change": () => store.setMixerDockSurface(sourceSelect.value === "" ? undefined : sourceSelect.value) });
    const mixLabel = h("label", {}, "Mix", mixSelect);
    const sourceLabel = h("label", {}, "Show", sourceSelect);
    const rows = h("div", { class: "rows", "data-testid": "remote-mix" });
    mixSelect.addEventListener("change", () => {
      channels.meteredMix.value = Number(mixSelect.value);
    });

    this.watch(() => {
      const chosen = store.mixerDockSurface.value;
      const surfaces = store.surfaces.list.value;
      sourceSelect.replaceChildren(h("option", { value: "" }, "This device"), ...surfaces.map((surface) => h("option", { value: surface.id }, surface.name)));
      sourceSelect.value = chosen ?? "";
      sourceSelect.disabled = !store.connected.value;
      // Only worth a menu when there is a surface to choose.
      sourceLabel.hidden = surfaces.length === 0;
      mixLabel.hidden = chosen !== undefined;
    });
    this.watch(() => {
      mixSelect.replaceChildren(...Array.from({ length: channels.mixCount }, (_, mix) => h("option", { value: String(mix) }, channels.mixName(mix))));
      mixSelect.value = String(channels.meteredMix.value);
    });

    // What the section holds on to, as the dock does: released when it shows something else.
    let held: (() => void)[] = [];
    const release = () => {
      for (const dispose of held.splice(0)) dispose();
      rows.replaceChildren();
    };
    this.onDisconnect(release);
    let shown: string | undefined;
    this.watch(() => {
      const surface = store.mixerDockSurface.value;
      const key = surface ?? "";
      if (key === shown) return;
      shown = key;
      untracked(() => {
        release();
        held = surface === undefined ? this.#followDevice(deviceId, rows) : this.#followSurface(surface, rows);
      });
    });
    return h("div", {}, h("div", { class: "choices" }, sourceLabel, mixLabel), rows);
  }

  /** The device's mix as rows, rebuilt when its channels or the mix change. */
  #followDevice(deviceId: string, rows: HTMLElement): (() => void)[] {
    const store = useStore();
    const channels = store.channels(deviceId);
    let rendered = "";
    return [
      ...followMix(store, deviceId),
      effect(() => {
        const mix = channels.meteredMix.value;
        const strips = channels.inMix(mix).map((ch) => channelStrip(deviceId, mix, ch.slot, channels.strip(ch, mix)));
        const mixName = channels.mixName(mix);
        const key = JSON.stringify([mix, mixName, strips]);
        if (key === rendered) return;
        rendered = key;
        // Untracked: a row renders as it is appended, and what it reads must not rebuild the list.
        untracked(() => {
          if (strips.length === 0) {
            rows.replaceChildren(h("p", { class: "empty" }, `No channels in ${mixName}. `, h("a", { href: href({ page: "mixer", id: deviceId }), "data-explain": "dock.open-mixer" }, "Open the Mixer page")));
            return;
          }
          const master = h("ga-remote-strip", { "device-id": deviceId, mixer: String(mix), strip: "master", label: mixName });
          rows.replaceChildren(...strips.map((attributes) => h("ga-remote-strip", attributes)), master);
        });
      }),
    ];
  }

  /** A surface's own strips, as the dock shows them. */
  #followSurface(surfaceId: string, rows: HTMLElement): (() => void)[] {
    const store = useStore();
    let alive = true;
    let rendered = "";
    const paint = (name: string, ids: string[]): void => {
      if (!alive) return;
      if (ids.length === 0) {
        rows.replaceChildren(h("p", { class: "empty" }, `${name} has no strips yet. `, h("a", { href: href({ page: "surface", id: surfaceId }), "data-explain": "dock.open-surface" }, "Add some on the surface.")));
        return;
      }
      if (!isReady("ga-surface-strip")) {
        rows.replaceChildren(h("p", { class: "empty" }, `Loading ${name}…`));
        void loadElement("ga-surface-strip").then(
          () => paint(name, ids),
          () => {
            if (alive) rows.replaceChildren(h("p", { class: "empty", role: "alert" }, `${name} could not be loaded. Reload the page to try again.`));
          },
        );
        return;
      }
      rows.replaceChildren(h("div", { class: "surface", "data-testid": "remote-surface" }, ids.map((id) => h("ga-surface-strip", { "surface-id": surfaceId, "strip-id": id, compact: "" }))));
    };
    return [
      effect(() => {
        const surface = store.surfaces.surface(surfaceId);
        const ids = (surface?.strips ?? []).map((s) => s.id);
        const key = JSON.stringify(ids);
        if (key === rendered) return;
        rendered = key;
        untracked(() => paint(surface?.name ?? "This surface", ids));
      }),
      () => (alive = false),
    ];
  }

  /** Each preamp's card, with its type shown beside its name. */
  #inputs(deviceId: string): HTMLElement {
    const store = useStore();
    const inputs = store.inputs(deviceId);
    this.onDisconnect(inputs.activate());
    const enabled = () => store.connected.peek();
    const host: ControlHost = { watch: (fn) => this.watch(fn), onDisconnect: (fn) => this.onDisconnect(fn) };
    const cards = Array.from({ length: inputs.preampCount }, (_, i) => {
      const card = preampCard(host, inputs, i, enabled, { links: false });
      const type = h("span", { class: "type", "data-testid": `remote-type-${i}`, "data-explain": "remote.preamp-type", "data-explain-name": `Preamp ${i + 1}` });
      card.querySelector(".badges")?.prepend(type);
      this.watch(() => {
        const current = inputs.preamp(i).value.type;
        type.textContent = PREAMP_TYPES.find((t) => t.value === current)?.label ?? "";
        // A report of no type the app knows shows nothing, rather than an empty chip.
        type.hidden = type.textContent === "";
      });
      return card;
    });
    const box = h("div", { class: "preamps", "data-testid": "remote-preamps" }, cards);
    if (cards.length === 0) box.replaceChildren(h("p", { class: "placeholder" }, "This device has no preamps."));
    this.watch(() => {
      const connected = store.connected.value;
      for (const button of box.querySelectorAll<HTMLButtonElement>("button[data-control]")) button.disabled = !connected || button.hasAttribute("data-unavailable");
      for (const control of box.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!connected));
    });
    return box;
  }
}

/**
 * <ga-remote-strip device-id="…" mixer="0" strip="3|master" [label] [color] [input-group input-channel]>:
 * one channel of a mix as a row: its name and colour, mute, solo and reset, then a horizontal fader
 * the width of the screen with the channel's input meter along its foot. The master has no solo
 * and no meter, as its strip has none. Levels, scale and meters are the mixer strip's own
 * (`MixerModel`, `stripMeter`), so the row, the dock and the Mixer page stay in step.
 * Attributes are read when it renders: change them by replacing it.
 */
export class GaRemoteStrip extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; }
      .row { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 6px; padding: 8px; border-radius: 4px; background: var(--ga-surface-raised); }
      :host([strip="master"]) .row { border: 1px solid var(--ga-border-subtle); }
      .name {
        display: flex;
        align-items: center;
        gap: 8px;
        min-width: 0;
        font-family: "Josefin Sans Variable", system-ui, sans-serif;
        font-size: 16px;
        font-weight: 600;
      }
      .name::before { content: ""; flex: none; width: 6px; align-self: stretch; border-radius: 2px; background: var(--strip-colour, var(--ga-section-header)); }
      .name span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .doubled { padding: 0 5px; border-radius: 3px; background: var(--ga-notice-warning); color: var(--ga-surface-inset); font-family: "Inter Variable", system-ui, sans-serif; font-size: 11px; font-weight: 600; }
      .buttons { display: flex; gap: 6px; }
      .buttons button { min-width: 44px; min-height: 44px; padding: 0 8px; font-size: 14px; font-weight: 700; }
      .mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .solo[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
      .reset { font-size: 12px; }
      .reset[data-armed] { outline: 2px dashed var(--ga-accent); outline-offset: -2px; }
      .fader {
        position: relative;
        grid-column: 1 / -1;
        height: 48px;
        overflow: hidden;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 4px;
        background: var(--ga-surface-inset);
        cursor: ew-resize;
        outline: none;
      }
      .fader:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .fader[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .fill { position: absolute; top: 0; bottom: 8px; left: 0; background: var(--ga-accent); opacity: 0.35; }
      .handle {
        position: absolute;
        top: 3px;
        bottom: 11px;
        width: ${HANDLE_PX}px;
        left: calc((100% - ${HANDLE_PX}px) * var(--position, 0));
        border: 1px solid rgb(0 0 0 / 0.35);
        border-radius: 3px;
        background: linear-gradient(90deg, var(--ga-fader-cap-active), var(--ga-fader-cap));
        box-shadow: 0 1px 3px rgb(0 0 0 / 0.45);
      }
      .handle::after { content: ""; position: absolute; top: 3px; bottom: 3px; left: 50%; width: 2px; margin-left: -1px; background: rgb(0 0 0 / 0.45); }
      .value { position: absolute; top: 0; bottom: 8px; left: 0; right: 0; font-size: 15px; line-height: 38px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
      /* The input meter along the fader's foot, plasma style as the strips' are. */
      .meter { position: absolute; left: 0; right: 0; bottom: 0; height: 6px; overflow: hidden; background: var(--ga-meter-background); }
      .meter .gradient { position: absolute; inset: 0; background: var(--remote-meter-gradient, var(--ga-meter-gradient)); }
      .meter .mask { position: absolute; top: 0; bottom: 0; right: 0; width: 100%; background: var(--ga-meter-background); }
      .meter .peak-mark { position: absolute; top: 0; bottom: 0; width: 2px; margin-left: -1px; background: var(--ga-text-primary); opacity: 0.85; pointer-events: none; }
      .meter .peak-mark[hidden] { display: none; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const mixIndex = Number(this.getAttribute("mixer") ?? "0");
    const mixer = store.mixer(deviceId, mixIndex);
    const stripAttribute = this.getAttribute("strip") ?? "0";
    const id: StripId = stripAttribute === "master" ? "master" : Number(stripAttribute);
    const given = this.getAttribute("label") ?? "";
    const label = given !== "" ? given : id === "master" ? "Master" : `Strip ${id + 1}`;
    const testId = id === "master" ? "master" : String(id);
    const state = mixer.strip(id);
    const enabled = () => store.connected.peek();

    const fill = h("div", { class: "fill" });
    const handle = h("div", { class: "handle" });
    const value = h("span", { class: "value", "data-testid": `remote-level-${testId}` });
    const fader = h(
      "div",
      { class: "fader", role: "slider", tabindex: 0, "aria-label": `${label} level`, "aria-valuemin": -LEVEL_MAX, "aria-valuemax": 0, "data-testid": `remote-fader-${testId}`, "data-explain": id === "master" ? "remote.master-fader" : "remote.fader", "data-explain-name": label },
      fill,
      handle,
      value,
    );
    // The mixer strip's own taper, laid on its side: -inf at the left, 0 dB at the right.
    bindControl(fader, {
      axis: "x",
      min: LEVEL_MAX,
      max: 0,
      up: -1,
      page: 6,
      reset: SAFE_LEVEL,
      get: () => state.peek().level,
      set: (v) => mixer.setLevel(id, v),
      enabled,
      valueAt: (fraction) => levelAtFaderPosition(1 - fraction),
      positionOf: (level) => 1 - faderPosition(level),
      inset: HANDLE_PX / 2,
      level: levelReset(store.doubleClickUnity, (fn) => this.watch(fn)),
    });

    const mute = h("button", { type: "button", class: "mute", "data-testid": `remote-mute-${testId}`, "aria-label": `${label} mute`, "data-explain": id === "master" ? "strip.master-mute" : "strip.mute", "data-explain-name": label, "on:click": () => mixer.toggleMute(id) }, "M");
    // A reset a stray tap cannot make: it asks first, and goes to the safe level, never unity.
    const reset = h("button", { type: "button", class: "reset", "data-testid": `remote-reset-${testId}`, "aria-label": `Reset ${label} to -20 dB`, title: `Reset ${label} to -20 dB: tap twice`, "data-explain": "remote.reset", "data-explain-name": label }, "-20");
    const disarm = bindConfirm(reset, "-20", () => mixer.setLevel(id, SAFE_LEVEL));
    this.onDisconnect(disarm);
    const buttons = h("div", { class: "buttons" }, reset, mute);
    const name = h("div", { class: "name", title: label, "data-explain": "strip.name", "data-explain-name": label }, h("span", {}, label));

    if (id !== "master") {
      const solo = h("button", { type: "button", class: "solo", "data-testid": `remote-solo-${testId}`, "aria-label": `${label} solo`, "data-explain": "strip.solo", "data-explain-name": label, "on:click": () => mixer.toggleSolo(id) }, "S");
      buttons.append(solo);
      this.watch(() => {
        solo.setAttribute("aria-pressed", String(state.value.solo));
      });

      const inputGroup = this.getAttribute("input-group");
      const source = inputGroup === null ? undefined : { group: Number(inputGroup), channel: Number(this.getAttribute("input-channel") ?? "0") };
      const inputMeter = store.stripMeter(deviceId, mixIndex, id, source);
      const mask = h("div", { class: "mask" });
      const peakMark = h("div", { class: "peak-mark", hidden: "" });
      const meter = h("div", { class: "meter", "data-testid": `remote-meter-${testId}`, "data-explain": "strip.meter", "data-explain-name": label }, h("div", { class: "gradient" }), mask, peakMark);
      fader.append(meter);
      if (inputMeter === undefined) {
        meter.title = "This channel has no input, so there is nothing to meter";
      } else {
        // The app's one frame loop for meters, as every strip's: smoothed, with a held peak.
        this.onDisconnect(
          animateMeter(inputMeter.level, (motion) => {
            mask.style.width = `${100 - meterDeflection(motion.level)}%`;
            peakMark.hidden = motion.peak >= METER_FLOOR;
            peakMark.style.left = `${meterDeflection(motion.peak)}%`;
          }),
        );
        this.watch(() => {
          meter.title = inputMeter.note.value;
        });
      }

      // One signal reaching the mix twice, as the strips flag it.
      const doubled = store.doubledFeed(deviceId, mixIndex, id);
      const badge = h("span", { class: "doubled", "data-testid": `remote-doubled-${testId}`, hidden: "", "data-explain": "strip.doubled" }, "×2");
      name.append(badge);
      this.watch(() => {
        const message = doubled.value;
        badge.hidden = message === undefined;
        badge.title = message ?? "";
        badge.setAttribute("aria-label", message ?? "");
      });
      this.watch(() => {
        const colours = Math.max(1, store.theme.value.palette.length);
        const own = this.getAttribute("color");
        this.style.setProperty("--strip-colour", own !== null && own !== "" ? own : `var(--ga-channel-palette-${Math.floor(id / 2) % colours})`);
      });
    }

    this.root.replaceChildren(h("div", { class: "row", "data-testid": `remote-strip-${testId}` }, name, buttons, fader));

    this.watch(() => {
      const s = state.value;
      const position = 1 - faderPosition(s.level);
      handle.style.setProperty("--position", String(position));
      fill.style.width = `calc(${HANDLE_PX / 2}px + (100% - ${HANDLE_PX}px) * ${position})`;
      value.textContent = formatLevel(s.level);
      fader.setAttribute("aria-valuenow", String(-s.level));
      fader.setAttribute("aria-valuetext", formatLevel(s.level));
      mute.setAttribute("aria-pressed", String(s.mute));
    });
    this.watch(() => {
      const usable = store.connected.value;
      for (const button of this.root.querySelectorAll("button")) button.disabled = !usable;
      fader.setAttribute("aria-disabled", String(!usable));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-remote": GaRemote;
    "ga-remote-strip": GaRemoteStrip;
  }
}
