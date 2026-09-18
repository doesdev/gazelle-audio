// <ga-control-room>: the right zone's Control Room panel (decisions P56, P57). It shows the device on
// the current page, or the one last selected (P71), as a <ga-monitor device-id="…">, with what the
// user chose for it (2026-09-16): the outputs chosen on the Outputs page (Monitor, HP1 and HP2 until
// then; kept per device in the workspace), each with volume, mute, (Quadro) dim, Mono for the mix that
// feeds it, and a mono badge where the device reports one; on the Studio+, talkback: the hold-to-talk
// button, its level and where it goes. Outputs and talkback use the same OutputsModel as the Outputs
// page, and mono the same ChannelsModel as the mix masters, so they all stay in step.
//
// Mono (P57, per output since 2026-09-17): neither model can make an output mono, so an output's Mono
// sums the mix routed to it, which every other output playing that mix hears too; the button names
// them. Which mix feeds an output is known once its routing is read (P97). The panel reads nothing on
// its own, so until then the button reads the routing first; an output no mix feeds (or several do)
// has it disabled, with the reason as its title.

import { h } from "../core/dom.ts";
import type { OutputFeed } from "../store/channels.ts";
import { formatVolume, VOLUME_MAX, type OutputInfo } from "../store/outputs.ts";
import { bindControl, bindMomentary, levelReset } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { route } from "./router.ts";

/** Where a double-click puts a volume: -20 dB, the safe level every level resets to (the user, 2026-09-18). */
/** A volume's double-click: -30 dB, quieter than a fader's -20 (the user, 2026-09-18). */
const VOLUME_RESET = 30;

export class GaControlRoom extends GaElement {
  static override styles = [sheet(`:host { display: block; }`)];

  protected override render(): void {
    const store = useStore();
    let shown: string | undefined;
    let shownOutputs = "";
    this.watch(() => {
      const current = route.value;
      const known = store.devices.value.filter((d) => d.family !== null);
      const id = known.find((d) => d.id === current.id)?.id ?? store.deviceInView(true);
      // The outputs chosen on the Outputs page; a new choice builds the panel again.
      const outputs = id === undefined ? "" : store.controlRoomOutputs(id).value.join(",");
      if (id === shown && outputs === shownOutputs && this.root.childElementCount > 0) return;
      shown = id;
      shownOutputs = outputs;
      this.root.replaceChildren(id === undefined ? h("p", { class: "placeholder" }, "No device of known model is connected.") : h("ga-monitor", { "device-id": id }));
    });
  }
}

export class GaMonitor extends GaElement {
  static override styles = [
    sheet(`
      :host { display: grid; gap: 8px; }
      .device { font-size: 11px; color: var(--ga-text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .group { display: grid; gap: 4px; }
      .group + .group { padding-top: 8px; border-top: 1px solid var(--ga-border-subtle); }
      .head { display: flex; align-items: center; gap: 6px; min-width: 0; }
      .label { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; white-space: nowrap; }
      .caption { min-width: 0; overflow: hidden; font-size: 11px; color: var(--ga-text-secondary); white-space: nowrap; text-overflow: ellipsis; }
      .spacer { flex: 1; }
      .badge { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-inverse); background: var(--ga-accent); }
      .volume { position: relative; height: 24px; border: 1px solid var(--ga-border-subtle); border-radius: 3px; background: var(--ga-surface-inset); cursor: ew-resize; touch-action: none; outline: none; }
      .volume:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .volume .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
      .volume .value { position: absolute; inset: 0; font-size: 12px; line-height: 22px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
      .volume[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
      .buttons { display: flex; gap: 4px; }
      button { min-width: 40px; min-height: 22px; padding: 0 6px; font-size: 11px; font-weight: 700; }
      .mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .dim[aria-pressed="true"], .to[aria-pressed="true"], .mono[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .talk-row { display: grid; grid-template-columns: auto minmax(0, 1fr); align-items: center; gap: 6px; }
      .talk { min-height: 26px; }
      .talk[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
      .to-row { display: flex; align-items: center; gap: 4px; }
      .to-row .caption { flex: none; margin-right: 2px; }
      .to { flex: 1; min-width: 0; padding: 0 2px; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const outputs = store.outputs(deviceId);
    this.onDisconnect(outputs.activate());
    const enabled = () => store.connected.peek();
    const model = store.devices.peek().find((d) => d.id === deviceId)?.model ?? deviceId;

    /** A horizontal level slider on the outputs' scale: 0 dB at the right, -inf at the left. */
    const slider = (label: string, testId: string, get: () => number, set: (volume: number) => void, explain: string, name?: string) => {
      const fill = h("div", { class: "fill" });
      const value = h("span", { class: "value" });
      const element = h("div", { class: "volume", role: "slider", tabindex: 0, "aria-label": label, "aria-valuemin": -VOLUME_MAX, "aria-valuemax": 0, "data-testid": testId, "data-explain": explain, "data-explain-name": name }, fill, value);
      bindControl(element, { axis: "x", min: VOLUME_MAX, max: 0, up: -1, page: 6, reset: VOLUME_RESET, get, set, enabled, level: levelReset(store.doubleClickUnity, (fn) => this.watch(fn), 0, "-30 dB") });
      const show = (volume: number) => {
        fill.style.width = `${((VOLUME_MAX - Math.min(VOLUME_MAX, Math.max(0, volume))) / VOLUME_MAX) * 100}%`;
        value.textContent = formatVolume(volume);
        element.setAttribute("aria-valuenow", String(-volume));
        element.setAttribute("aria-valuetext", formatVolume(volume));
      };
      return { element, show };
    };

    // The outputs chosen on the Outputs page, in the device's order; the panel is built again when they change.
    const rows = store.controlRoomOutputs(deviceId).peek().flatMap((id) => {
      const output = outputs.outputs[id];
      if (output === undefined) return [];
      const state = outputs.state(id);
      const volume = slider(`${output.name} volume`, `cr-volume-${id}`, () => state.peek().volume, (v) => outputs.setVolume(id, v), "cr.volume", output.name);
      const mute = h("button", { type: "button", class: "mute", "data-testid": `cr-mute-${id}`, "aria-label": `${output.name} mute`, "data-explain": "cr.mute", "data-explain-name": output.name, "on:click": () => outputs.setMute(id, !state.peek().mute) }, "Mute");
      const dim = output.dim ? h("button", { type: "button", class: "dim", "data-testid": `cr-dim-${id}`, "aria-label": `${output.name} dim`, "data-explain": "cr.dim", "data-explain-name": output.name, "on:click": () => outputs.setDim(id, !state.peek().dim) }, "Dim") : undefined;
      // Mono is reported (Quadro) but has no command, so it is a badge; the button sums the mix that feeds the output.
      const mono = h("span", { class: "badge", title: `The device reports ${output.name} in mono`, hidden: true, "data-explain": "cr.mono-badge", "data-explain-name": output.name }, "MONO");
      const sum = this.#mono(output);
      this.watch(() => {
        const s = state.value;
        volume.show(s.volume);
        mute.setAttribute("aria-pressed", String(s.mute));
        dim?.setAttribute("aria-pressed", String(s.dim));
        mono.hidden = !s.mono;
      });
      return [h("div", { class: "group", "data-testid": `cr-output-${id}` }, h("div", { class: "head" }, h("span", { class: "label" }, output.name), mono, sum.caption, h("span", { class: "spacer" }), h("div", { class: "buttons" }, mute, dim, sum.button)), volume.element)];
    });

    // Talkback: the Studio+ only. The Quadro's panel has no talkback commands, so it shows nothing of it.
    let talkback: HTMLElement | undefined;
    if (outputs.talkback !== undefined) {
      const talk = h("button", { type: "button", class: "talk", "data-testid": "cr-talk", "aria-label": "Talkback (hold to talk)", title: "Hold to talk", "data-explain": "cr.talk" }, "Talk");
      bindMomentary(talk, (on) => outputs.setTalk(on), enabled);
      const level = slider("Talkback level", "cr-talk-volume", () => outputs.talk.peek().volume, (v) => outputs.setTalkbackVolume(v), "cr.talk-level");
      const destinations = outputs.talkback.destinations.map((d) =>
        h("button", { type: "button", class: "to", "data-testid": `cr-talk-to-${d.id}`, "aria-label": `Talkback to ${d.name}`, "data-explain": "cr.talk-to", "data-explain-name": d.name, "on:click": () => outputs.setTalkbackTo(d.id, !(outputs.talk.peek().to[d.id] ?? false)) }, d.name),
      );
      this.watch(() => {
        const t = outputs.talk.value;
        talk.setAttribute("aria-pressed", String(t.on));
        level.show(t.volume);
        for (const [i, button] of destinations.entries()) button.setAttribute("aria-pressed", String(t.to[i] ?? false));
      });
      talkback = h(
        "div",
        { class: "group", "data-testid": "cr-talkback" },
        h("div", { class: "head" }, h("span", { class: "label" }, "Talkback")),
        h("div", { class: "talk-row" }, talk, level.element),
        h("div", { class: "to-row" }, h("span", { class: "caption" }, "To"), destinations),
      );
    }

    this.root.replaceChildren(
      h("div", { class: "device", title: model, "data-explain": "cr.device" }, model),
      ...rows,
      ...(talkback === undefined ? [] : [talkback]),
    );

    this.watch(() => {
      const connected = store.connected.value;
      // Mono buttons also depend on what feeds their output, so they follow the connection themselves.
      for (const button of this.root.querySelectorAll<HTMLButtonElement>("button:not(.mono)")) button.disabled = !connected;
      for (const control of this.root.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!connected));
    });
  }

  /**
   * An output's Mono button and the caption saying what feeds it. The button sums the one mix routed
   * to the output (P57), naming the other outputs that play it; with no mix, or several, it is
   * disabled and its title says why. Until the output's routing is read it reads every output's
   * routing first (once, P97), then the mixes if unread, since the pans mono keeps must be the device's.
   */
  #mono(output: OutputInfo): { button: HTMLButtonElement; caption: HTMLElement } {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const channels = store.channels(deviceId);
    const topology = store.topology(deviceId);
    const destination = topology?.outputs.findIndex((g) => g.id === output.group) ?? -1;
    const feed = destination < 0 ? undefined : channels.outputFeed(destination);
    const button = h("button", { type: "button", class: "mono", "data-testid": `cr-mono-${output.id}`, "data-explain": "cr.mono", "data-explain-name": output.name }, "Mono");
    const caption = h("span", { class: "caption", "data-testid": `cr-feed-${output.id}`, "data-explain": "cr.feed", "data-explain-name": output.name });
    const mixLabel = (mix: number) => {
      const name = channels.layout.value.mixes[mix]?.name;
      return name ? `Mix ${mix + 1}: ${name}` : `Mix ${mix + 1}`;
    };
    // A pair's label in the device's words ("HP1", "USB REC 1/2"), with the panel's own names for its outputs.
    const outputs = store.outputs(deviceId).outputs;
    const pairName = (label: string) => {
      for (const o of outputs) {
        const group = topology?.outputs.find((g) => g.id === o.group)?.name;
        if (group !== undefined && (label === group || label.startsWith(`${group} `))) return `${o.name}${label.slice(group.length)}`;
      }
      return label;
    };

    button.addEventListener("click", async () => {
      if (feed === undefined) return;
      if (feed.peek().state === "unread") await store.readRoutes(deviceId, [...new Set(channels.outputPairs().map((p) => p.destination))]);
      const now = feed.peek();
      if (now.state !== "mixes" || now.mixes.length !== 1) return;
      const mix = now.mixes[0] as number;
      const on = !channels.isMono(mix);
      if (store.mixesToRead(deviceId)) await store.readMixes(deviceId);
      channels.setMono(mix, on);
    });

    this.watch(() => {
      const state: OutputFeed = feed?.value ?? { state: "unknown" };
      const connected = store.connected.value;
      let usable = false;
      let pressed = false;
      let label = `${output.name} mono`;
      switch (state.state) {
        case "unread":
          usable = true;
          caption.textContent = "";
          button.title = `Reads the routing first, to find the mix that feeds ${output.name}, then sums that mix to mono.`;
          break;
        case "unknown":
          caption.textContent = "";
          button.title = `The routing to ${output.name} could not be read, so the mix that feeds it is not known.`;
          break;
        case "none":
          caption.textContent = state.sources.length === 0 ? "Muted" : list(state.sources);
          button.title = `No mix feeds ${output.name}: ${state.sources.length === 0 ? "it is muted in routing" : `it plays ${list(state.sources)}`}, so there is no mix to sum to mono.`;
          break;
        case "mixes": {
          const mixes = state.mixes.map(mixLabel);
          caption.textContent = list(mixes);
          if (state.mixes.length > 1) {
            button.title = `${output.name} plays ${list(mixes)}: sum each to mono with the Mono on its master, on the Mixer page.`;
            break;
          }
          const others = state.others.map(pairName);
          usable = true;
          pressed = channels.isMono(state.mixes[0] as number);
          button.title = `Sums ${mixes[0]} to mono${others.length > 0 ? `, so ${list(others)} ${others.length === 1 ? "goes" : "go"} mono too` : ""}: pans its channels to centre, and restores them when turned off.`;
          label = `${output.name} mono (${mixes[0]}${others.length > 0 ? `, also ${list(others)}` : ""})`;
          break;
        }
      }
      button.disabled = !connected || !usable;
      button.setAttribute("aria-pressed", String(pressed));
      button.setAttribute("aria-label", label);
      caption.title = caption.textContent ?? "";
    });
    return { button, caption };
  }
}

/** "A", "A and B", "A, B and C". */
function list(items: readonly string[]): string {
  return items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items.at(-1)}`;
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-control-room": GaControlRoom;
    "ga-monitor": GaMonitor;
  }
}
