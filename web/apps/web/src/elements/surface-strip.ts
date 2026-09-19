// <ga-surface-strip surface-id="…" strip-id="…" [compact]>: one strip of a cross-device surface,
// shared by the surface page and the mixer dock. Its top is a badge in its
// device's colour naming the device, so two devices' strips side by side never read as one mixer.
// Below it are that device's own controls, built by the same code as the device's pages:
// - a channel: the Mixer page's strip (`ga-strip`, attributes from `channelStrip`) for the channel in
//   the surface's mix for its device, or its pinned mix, which the caption names;
// - a master: the mix's master strip;
// - an input: the Inputs page's preamp card or digital input cell, and the input's meter;
// - an output: the Outputs page's row;
// - a port: a digital output (S/PDIF, or 8 channels of ADAT), which has no level of its own on either
//   model: what feeds each pair, a menu to route a mix, a source or nothing there instead (a
//   real routing change on the device that owns the port), and the master of each mix that
//   feeds it, which is that output's level;
// - a label: its text.
// A channel on a digital input, or a digital input strip, that a declared cable feeds says where its
// signal comes from ("from Drum rack ADAT out 3 ← PREAMP 3"). The Studio+'s S/PDIF input strips have
// its S/PDIF SRC switch, which decides whether it must follow the sender's clock.
// A strip for a device that is not attached, or a channel that no longer exists, says so and keeps
// its place. `compact` is the dock's size: slim strips and no captions.

import { h } from "../core/dom.ts";
import { effect, untracked } from "../core/signal.ts";
import { INPUT_TYPES } from "../store/surfaces.ts";
import { channelSpan, portName, portWidth, type InputPort, type RouteChoice } from "../store/cables.ts";
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
      :host([kind="input"]), :host([kind="output"]), :host([kind="port"]) { flex: 0 0 156px; }
      :host([kind="label"]) { flex: 0 0 56px; }
      :host([compact][kind="channel"]) { flex-basis: 48px; }
      :host([compact][kind="master"]) { flex-basis: 52px; }
      :host([compact][kind="input"]), :host([compact][kind="output"]), :host([compact][kind="port"]) { flex-basis: 132px; }
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
      .provenance { flex: none; padding: 2px 4px; border-radius: 2px; background: var(--ga-surface-inset); font-size: 9px; color: var(--ga-text-secondary); overflow-wrap: anywhere; }
      .provenance[hidden] { display: none; }
      .pairs { display: grid; gap: 3px; padding: 4px; border-radius: 3px; background: var(--ga-surface-raised); }
      .pair { display: grid; grid-template-columns: auto minmax(0, 1fr); align-items: center; gap: 2px 4px; font-size: 10px; }
      .pair .from { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
      .pair .from[data-direct] { color: var(--ga-accent); }
      .pair select { grid-column: 1 / -1; min-width: 0; min-height: 20px; padding: 0 2px; font-size: 10px; }
      .note { margin: 0; font-size: 9px; color: var(--ga-text-muted); }
      .feeds { display: flex; flex-direction: column; flex: 1; gap: 2px; min-height: 0; }
      .feeds .caption { white-space: normal; }
      .src { min-height: 20px; font-size: 10px; font-weight: 700; }
      .src[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .gone { margin: 0; padding: 6px 4px; border: 1px dashed var(--ga-border-strong); border-radius: 3px; font-size: 10px; color: var(--ga-text-muted); }
      .text-strip { display: flex; flex: 1; align-items: flex-start; justify-content: center; padding-top: 6px; border-radius: 3px; background: var(--ga-surface-raised); font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; writing-mode: vertical-rl; }
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
    const badge = h("div", { class: "badge", "data-testid": "device-badge", "data-explain": "surface.badge" });
    const caption = h("div", { class: "caption", "data-testid": "strip-caption", "data-explain": "surface.caption" });
    const provenance = h("div", { class: "provenance", "data-testid": "provenance", hidden: true, "data-explain": "surface.provenance" });
    const body = h("div", { class: "body" });
    this.root.replaceChildren(badge, caption, provenance, body);

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
        badge.setAttribute("data-explain-name", badge.textContent);
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
      if (known && deviceId !== undefined && strip.kind === "port" && strip.port !== undefined) {
        const first = strip.first ?? 0;
        const width = Math.min(portWidth(strip.port), store.cables.portChannels(deviceId, strip.port) - first);
        text = strip.port === "SPDIF_OUT" ? portName(strip.port) : `${portName(strip.port)} ${channelSpan(first + 1, first + width)}`;
        // The masters beside the port are rebuilt when the mixes feeding it change.
        shape.push(store.cables.feed(deviceId, strip.port, first).flatMap((pair) => (pair.mix === undefined ? [] : [[pair.mix, store.channels(deviceId).mixName(pair.mix)]])));
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

    // Where a cable brings this strip's signal from, and the sender's routing it needs, read once.
    this.watch(() => {
      const strip = store.surfaces.surface(surfaceId)?.strips.find((s) => s.id === stripId);
      const input = strip === undefined || strip.device_id === undefined ? undefined : this.#digitalInput(strip);
      if (strip?.device_id === undefined || input === undefined) {
        provenance.hidden = true;
        return;
      }
      const from = store.cables.provenance(strip.device_id, input.port, input.channel);
      provenance.hidden = from === undefined;
      provenance.textContent = from?.text ?? "";
      const read = store.cables.sendersToRead(strip.device_id, input.port, input.channel);
      if (read !== undefined && store.routesToRead(read.deviceId, [read.destination])) untracked(() => void store.readRoutes(read.deviceId, [read.destination]));
    });
  }

  /** The digital input a strip shows, directly or as a channel's source, or undefined. Reactive. */
  #digitalInput(strip: SurfaceStrip): { port: InputPort; channel: number } | undefined {
    const store = useStore();
    const deviceId = strip.device_id;
    if (deviceId === undefined || store.topology(deviceId) === undefined) return undefined;
    if (strip.kind === "input" && strip.input !== undefined && (strip.input.kind === "adat" || strip.input.kind === "spdif")) {
      return { port: strip.input.kind === "adat" ? "ADAT_IN" : "SPDIF_IN", channel: strip.input.channel };
    }
    if (strip.kind !== "channel") return undefined;
    const source = store.channels(deviceId).layout.value.channels.find((c) => c.id === strip.channel)?.source;
    const type = source === undefined ? undefined : store.topology(deviceId)?.inputs[source.group]?.type;
    return source !== undefined && (type === "ADAT_IN" || type === "SPDIF_IN") ? { port: type, channel: source.channel } : undefined;
  }

  #body(host: ControlHost, strip: SurfaceStrip, deviceId: string | undefined, known: boolean, mix: number, compact: boolean): HTMLElement[] {
    const store = useStore();
    if (strip.kind === "label") return [h("div", { class: "text-strip", "data-testid": "strip-label" }, strip.text ?? "")];
    if (deviceId === undefined || !known) return [h("p", { class: "gone", "data-testid": "strip-gone" }, "Not connected")];
    const enabled = () => store.connected.peek();

    if (strip.kind === "channel" || strip.kind === "master") {
      const channels = store.channels(deviceId);
      // Levels are the device's, read once as the Mixer page reads them; meters follow the report.
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
      const extras: HTMLElement[] = [];
      if (kind === "spdif" && store.hasSpdifSrc(deviceId)) {
        // The Studio+'s sample-rate converter on its S/PDIF input: on, it need not follow the sender's clock.
        const src = h("button", { type: "button", class: "src", "data-control": "", "data-testid": "spdif-src", "data-explain": "surface.src", title: "S/PDIF SRC: convert the incoming rate, so this device need not follow the sender's clock", "on:click": () => store.setSpdifSrc(deviceId, !store.spdifSrc(deviceId)) }, "SRC");
        host.watch(() => src.setAttribute("aria-pressed", String(store.spdifSrc(deviceId) === true)));
        extras.push(src);
      }
      if (meter === undefined) return [control, ...extras];
      // The input's signal as it arrives, on the meters' scale, so a strip shows whether anything is there.
      const mask = h("div", { class: "mask" });
      const level = h("div", { class: "level", "data-testid": "input-level", title: "The input's level", "data-explain": "surface.input-level" }, mask);
      host.onDisconnect(
        animateMeter(meter.level, (motion) => {
          mask.style.width = `${100 - (motion.level >= METER_FLOOR ? 0 : meterDeflection(motion.level))}%`;
        }),
      );
      return [control, level, ...extras];
    }

    if (strip.kind === "output" && strip.output !== undefined) {
      const outputs = store.outputs(deviceId);
      host.onDisconnect(outputs.activate());
      const output = outputs.outputs.find((o) => o.id === strip.output);
      return output === undefined ? [h("p", { class: "gone" }, "No such output")] : [outputRow(host, outputs, output, enabled)];
    }

    if (strip.kind === "port" && strip.port !== undefined) return this.#port(host, deviceId, strip.port, strip.first ?? 0, compact);
    return [];
  }

  /**
   * A digital output: each pair with what feeds it and a menu to route something else there, a
   * meter where the device reports one, and the master of each mix that feeds it.
   */
  #port(host: ControlHost, deviceId: string, port: "SPDIF_OUT" | "ADAT_OUT", first: number, compact: boolean): HTMLElement[] {
    const store = useStore();
    const cables = store.cables;
    const destination = cables.position(deviceId, port);
    const device = store.devices.peek().find((d) => d.id === deviceId);
    const name = device === undefined ? deviceId : displayName(device, store.workspace.peek());
    // What feeds the port is read once, as the Routing page reads it.
    host.watch(() => {
      if (store.routesToRead(deviceId, [destination])) untracked(() => void store.readRoutes(deviceId, [destination]));
    });

    const pairs = h("div", { class: "pairs", "data-testid": "port-pairs" });
    const note = h("p", { class: "note", "data-testid": "port-note" });
    const choices = cables.routeChoices(deviceId);
    const rows = cables.feed(deviceId, port, first).map((pair) => {
      const from = h("span", { class: "from", "data-testid": `port-feed-${pair.channel}`, "data-explain": "surface.port-feed" });
      const menu = h(
        "select",
        {
          "aria-label": `Route to ${portName(port)} ${pair.label}`,
          "data-testid": `port-route-${pair.channel}`,
          "data-explain": "surface.port-route",
          // A menu of actions, not a value: a wheel step would route something.
          "data-no-wheel": true,
          "on:change": () => {
            const value = menu.value;
            menu.value = "";
            const [what, a, b] = value.split(":");
            const choice: RouteChoice | undefined = what === "mix" ? { mix: Number(a) } : what === "src" ? { group: Number(a), channel: Number(b) } : what === "mute" ? { mute: true } : undefined;
            if (choice === undefined) return;
            try {
              void cables.routePair(deviceId, port, pair.channel, choice);
            } catch (error) {
              store.reportError(error instanceof Error ? error.message : String(error));
            }
          },
        },
        h("option", { value: "" }, "Route…"),
        h("optgroup", { label: "Mixes" }, choices.mixes.map((m) => h("option", { value: `mix:${m.mix}` }, m.label))),
        h("optgroup", { label: "Sources, bit for bit" }, choices.sources.map((s) => h("option", { value: `src:${s.group}:${s.channel}` }, s.label))),
        h("option", { value: "mute" }, "Mute"),
      );
      return { channel: pair.channel, row: h("div", { class: "pair" }, h("span", { class: "muted" }, pair.label), from, menu), from, menu };
    });
    pairs.append(...rows.map((r) => r.row));
    host.watch(() => {
      const feed = cables.feed(deviceId, port, first);
      for (const row of rows) {
        const pair = feed.find((p) => p.channel === row.channel);
        row.from.textContent = pair === undefined ? "" : pair.known ? `← ${pair.text}` : "← not read";
        row.from.title = pair?.direct === true ? `${pair.text}, bit for bit` : (pair?.text ?? "");
        row.from.toggleAttribute("data-direct", pair?.direct === true);
      }
      note.textContent = feed.some((p) => p.direct) ? `Bit for bit: no level on ${name}.` : feed.every((p) => !p.known) ? "Routing not read (dry run reads nothing)." : "";
      note.hidden = note.textContent === "" || compact;
    });
    host.watch(() => {
      const connected = store.connected.value;
      for (const row of rows) row.menu.disabled = !connected;
    });

    const parts: HTMLElement[] = [pairs, note];
    // Only the Quadro reports its S/PDIF output's level in a fixed field.
    if (port === "SPDIF_OUT" && store.topology(deviceId)?.family === "quadro") {
      const field = store.field(deviceId, "0x73", "peaks_spdif_out");
      host.onDisconnect(store.watchReport(deviceId, "0x73"));
      for (const side of [0, 1]) {
        const mask = h("div", { class: "mask" });
        parts.push(h("div", { class: "level", "data-testid": `port-level-${side}`, title: side === 0 ? "Left" : "Right", "data-explain": "surface.port-level" }, mask));
        let level = METER_FLOOR;
        host.watch(() => {
          const bytes = field.value;
          level = (bytes instanceof Uint8Array || Array.isArray(bytes)) && side < bytes.length ? Number((bytes as ArrayLike<number>)[side]) : METER_FLOOR;
          mask.style.width = `${100 - (level >= METER_FLOOR ? 0 : meterDeflection(level))}%`;
        });
      }
    }

    const mixes = [...new Set(cables.feed(deviceId, port, first).flatMap((p) => (p.mix === undefined ? [] : [p.mix])))];
    if (mixes.length > 0) {
      const channels = store.channels(deviceId);
      host.watch(() => {
        if (store.mixesToRead(deviceId)) untracked(() => void store.readMixes(deviceId));
      });
      const feeds = h("div", { class: "feeds", "data-testid": "port-masters" });
      for (const mix of mixes) {
        host.onDisconnect(store.mixer(deviceId, mix).activate());
        const label = channels.mixName(mix);
        feeds.append(
          h("div", { class: "caption", "data-testid": `port-master-caption-${mix}`, "data-explain": "surface.port-master" }, `${label} master, feeds ${portName(port)}`),
          h("ga-strip", { "device-id": deviceId, mixer: String(mix), strip: "master", label, compact: compact ? "" : undefined }),
        );
      }
      parts.push(feeds);
    }
    return parts;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-surface-strip": GaSurfaceStrip;
  }
}
