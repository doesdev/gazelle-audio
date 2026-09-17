// <ga-surface-strip surface-id="…" strip-id="…" [compact]>: one strip of a cross-device surface
// (workspace spec §4), shared by the surface page and the mixer dock. Its top is a badge in its
// device's colour naming the device, so two devices' strips side by side never read as one mixer.
// Below it are that device's own controls, built by the same code as the device's pages:
// - a channel: the Mixer page's strip (`ga-strip`, attributes from `channelStrip`) for the channel in
//   the surface's mix for its device, or its pinned mix, which the caption names;
// - a master: the mix's master strip;
// - an input: the Inputs page's preamp card or digital input cell, and the input's meter;
// - an output: the Outputs page's row;
// - a label: its text.
// A strip for a device that is not attached, or a channel that no longer exists, says so and keeps
// its place. `compact` is the dock's size: slim strips and no captions.

import { h } from "../core/dom.ts";
import { effect, untracked } from "../core/signal.ts";
import { INPUT_TYPES } from "../store/surfaces.ts";
import { displayName, type SurfaceStrip } from "../store/store.ts";
import { channelStrip } from "./channel.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { digitalCell, INPUT_CONTROL_STYLES, preampCard, type ControlHost } from "./inputs-page.ts";
import { animateMeter, METER_FLOOR } from "./meter-motion.ts";
import { meterDeflection } from "../store/mixer.ts";
import { OUTPUT_CONTROL_STYLES, outputRow } from "./outputs-page.ts";

export class GaSurfaceStrip extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 3px; min-width: 0; min-height: 0; }
      :host([kind="channel"]), :host([kind="master"]) { flex: 0 0 76px; }
      :host([kind="input"]), :host([kind="output"]) { flex: 0 0 156px; }
      :host([kind="label"]) { flex: 0 0 56px; }
      :host([compact][kind="channel"]) { flex-basis: 48px; }
      :host([compact][kind="master"]) { flex-basis: 52px; }
      :host([compact][kind="input"]), :host([compact][kind="output"]) { flex-basis: 132px; }
      .badge {
        flex: none;
        overflow: hidden;
        padding: 1px 4px;
        border-left: 4px solid var(--device-colour, var(--ga-border-strong));
        border-radius: 2px;
        background: color-mix(in srgb, var(--device-colour, transparent) 30%, var(--ga-surface-raised));
        color: var(--ga-text-primary);
        font-size: 10px;
        font-weight: 700;
        white-space: nowrap;
        text-overflow: ellipsis;
      }
      :host([compact]) .badge { padding: 0 2px; border-left-width: 3px; font-size: 9px; }
      .caption { flex: none; overflow: hidden; font-size: 9px; color: var(--ga-text-muted); white-space: nowrap; text-overflow: ellipsis; }
      .body { display: flex; flex-direction: column; flex: 1; gap: 4px; min-height: 0; }
      .body ga-strip { flex: 1; min-height: 0; }
      .gone { margin: 0; padding: 6px 4px; border: 1px dashed var(--ga-border-strong); border-radius: 3px; font-size: 10px; color: var(--ga-text-muted); }
      .label { display: flex; flex: 1; align-items: flex-start; justify-content: center; padding-top: 6px; border-radius: 3px; background: var(--ga-surface-raised); font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; writing-mode: vertical-rl; }
      .level { position: relative; flex: none; height: 6px; overflow: hidden; border-radius: 2px; background: var(--surface-meter-gradient, var(--ga-meter-background)); }
      .level .mask { position: absolute; top: 0; bottom: 0; right: 0; width: 100%; background: var(--ga-meter-background); }
      ${INPUT_CONTROL_STYLES}
      ${OUTPUT_CONTROL_STYLES}
      /* A strip is narrow: the preamp card fills it, and an output's volume takes its own line. */
      .preamp { grid-column: auto; }
      .output { grid-template-columns: minmax(0, 1fr) auto; gap: 6px; padding: 6px; }
      .output .volume { grid-column: 1 / -1; grid-row: 2; }
      .toggles button { min-width: 0; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const surfaceId = this.getAttribute("surface-id") ?? "";
    const stripId = this.getAttribute("strip-id") ?? "";
    const compact = this.hasAttribute("compact");
    const badge = h("div", { class: "badge", "data-testid": "device-badge" });
    const caption = h("div", { class: "caption", "data-testid": "strip-caption" });
    const body = h("div", { class: "body" });
    this.root.replaceChildren(badge, caption, body);

    // What the body was built with; it is rebuilt, and its effects released, only when that changes.
    let scope: (() => void)[] = [];
    const release = () => {
      for (const dispose of scope.splice(0)) dispose();
    };
    this.onDisconnect(release);
    const host: ControlHost = { watch: (fn) => void scope.push(effect(fn)), onDisconnect: (fn) => void scope.push(fn) };
    let built = "";
    // As the Inputs and Outputs pages do for their controls (ga-strip disables its own).
    const usable = (connected: boolean) => {
      for (const button of body.querySelectorAll<HTMLButtonElement>("button[data-control]")) button.disabled = !connected || button.hasAttribute("data-unavailable");
      for (const control of body.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!connected));
    };

    this.watch(() => {
      const strip = store.surfaces.surface(surfaceId)?.strips.find((s) => s.id === stripId);
      if (strip === undefined) {
        this.removeAttribute("kind");
        return;
      }
      this.setAttribute("kind", strip.kind);
      const deviceId = strip.device_id;
      const device = deviceId === undefined ? undefined : store.devices.value.find((d) => d.id === deviceId);
      const known = device !== undefined && device.family !== null;
      badge.hidden = deviceId === undefined;
      if (deviceId !== undefined) {
        badge.textContent = device === undefined ? deviceId : displayName(device, store.workspace.value);
        badge.title = device === undefined ? `${deviceId} (not connected)` : `${displayName(device, store.workspace.value)} (${device.id})`;
        const colour = store.surfaces.deviceColor(deviceId);
        if (colour === undefined) badge.style.removeProperty("--device-colour");
        else badge.style.setProperty("--device-colour", colour);
        badge.dataset["colour"] = colour ?? "";
      }

      const mix = store.surfaces.stripMix(surfaceId, strip);
      const shape: unknown[] = [strip, known, compact];
      let text = "";
      if (known && deviceId !== undefined && (strip.kind === "channel" || strip.kind === "master")) {
        const channels = store.channels(deviceId);
        const mixName = channels.mixName(mix);
        text = `${strip.kind === "master" ? `${mixName} master` : mixName}${strip.mix === undefined ? "" : " (pinned)"}`;
        if (strip.kind === "channel") {
          const channel = channels.layout.value.channels.find((c) => c.id === strip.channel);
          shape.push(mix, channel === undefined ? null : channelStrip(deviceId, mix, channel.slot, channels.strip(channel, mix), compact));
        } else {
          shape.push(mix, mixName);
        }
      }
      caption.textContent = text;
      caption.hidden = compact || text === "";

      const key = JSON.stringify(shape);
      if (key === built) return;
      built = key;
      untracked(() => {
        release();
        body.replaceChildren(...this.#body(host, strip, deviceId, known, mix, compact));
        usable(store.connected.peek());
      });
    });

    this.watch(() => usable(store.connected.value));
  }

  #body(host: ControlHost, strip: SurfaceStrip, deviceId: string | undefined, known: boolean, mix: number, compact: boolean): HTMLElement[] {
    const store = useStore();
    if (strip.kind === "label") return [h("div", { class: "label", "data-testid": "strip-label" }, strip.text ?? "")];
    if (deviceId === undefined || !known) return [h("p", { class: "gone", "data-testid": "strip-gone" }, "Not connected")];
    const enabled = () => store.connected.peek();

    if (strip.kind === "channel" || strip.kind === "master") {
      const channels = store.channels(deviceId);
      // Levels are the device's, read once as the Mixer page reads them (P80); meters follow the report.
      host.watch(() => {
        if (store.mixesToRead(deviceId)) untracked(() => void store.readMixes(deviceId));
      });
      host.onDisconnect(store.mixer(deviceId, mix).activate());
      if (strip.kind === "master") return [h("ga-strip", { "device-id": deviceId, mixer: String(mix), strip: "master", label: channels.mixName(mix), compact: compact ? "" : undefined })];
      const channel = channels.layout.peek().channels.find((c) => c.id === strip.channel);
      if (channel === undefined) return [h("p", { class: "gone", "data-testid": "strip-gone" }, "Channel removed")];
      return [h("ga-strip", channelStrip(deviceId, mix, channel.slot, channels.strip(channel, mix), compact))];
    }

    if (strip.kind === "input" && strip.input !== undefined) {
      const inputs = store.inputs(deviceId);
      host.onDisconnect(inputs.activate());
      const { kind, channel } = strip.input;
      const control = kind === "preamp" ? preampCard(host, inputs, channel, enabled, { links: false }) : digitalCell(host, inputs, inputs.digital.find((g) => g.kind === kind) ?? { kind, label: kind, count: 0, editable: false, linkPairs: 0 }, channel, enabled, { links: false });
      const topology = store.topology(deviceId);
      const group = topology?.inputs.findIndex((g) => g.type === INPUT_TYPES[kind]) ?? -1;
      const meter = group < 0 ? undefined : store.inputMeter(deviceId, { group, channel });
      if (meter === undefined) return [control];
      // The input's signal as it arrives, on the meters' scale, so a strip shows whether anything is there.
      const mask = h("div", { class: "mask" });
      const level = h("div", { class: "level", "data-testid": "input-level", title: "The input's level" }, mask);
      host.onDisconnect(
        animateMeter(meter.level, (motion) => {
          mask.style.width = `${100 - (motion.level >= METER_FLOOR ? 0 : meterDeflection(motion.level))}%`;
        }),
      );
      return [control, level];
    }

    if (strip.kind === "output" && strip.output !== undefined) {
      const outputs = store.outputs(deviceId);
      host.onDisconnect(outputs.activate());
      const output = outputs.outputs.find((o) => o.id === strip.output);
      return output === undefined ? [h("p", { class: "gone" }, "No such output")] : [outputRow(host, outputs, output, enabled)];
    }
    return [];
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-surface-strip": GaSurfaceStrip;
  }
}
