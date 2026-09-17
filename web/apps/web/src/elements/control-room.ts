// <ga-control-room>: the right zone's Control Room panel (decisions P56, P57). It shows the device on
// the current page, or the one last selected (P71), as a <ga-monitor device-id="…">, with what the
// user chose for it (2026-09-16): Monitor, HP1 and HP2, each with volume, mute and (Quadro) dim and a
// mono badge where the device reports one; on the Studio+, talkback: the hold-to-talk button, its
// level and where it goes; and a mono switch for the device's selected mix. Outputs and talkback use
// the same OutputsModel as the Outputs page, and mono the same ChannelsModel as the mix masters, so
// they all stay in step.

import { h } from "../core/dom.ts";
import { formatVolume, VOLUME_MAX } from "../store/outputs.ts";
import { bindControl, bindMomentary } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { route } from "./router.ts";

/** The outputs the panel shows, by id on both models: MONITOR 0, HP1 1, HP2 2 (not LINE OUT or REAMP). */
const CONTROL_ROOM_OUTPUTS = [0, 1, 2] as const;
/** The vendor panels' starting volume, which a reset returns to. */
const VOLUME_RESET = 30;

export class GaControlRoom extends GaElement {
  static override styles = [sheet(`:host { display: block; }`)];

  protected override render(): void {
    const store = useStore();
    let shown: string | undefined;
    this.watch(() => {
      const current = route.value;
      const known = store.devices.value.filter((d) => d.family !== null);
      const id = known.find((d) => d.id === current.id)?.id ?? store.deviceInView(true);
      if (id === shown && this.root.childElementCount > 0) return;
      shown = id;
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
    const channels = store.channels(deviceId);
    this.onDisconnect(outputs.activate());
    const enabled = () => store.connected.peek();
    const model = store.devices.peek().find((d) => d.id === deviceId)?.model ?? deviceId;

    /** A horizontal level slider on the outputs' scale: 0 dB at the right, -inf at the left. */
    const slider = (label: string, testId: string, get: () => number, set: (volume: number) => void) => {
      const fill = h("div", { class: "fill" });
      const value = h("span", { class: "value" });
      const element = h("div", { class: "volume", role: "slider", tabindex: 0, "aria-label": label, "aria-valuemin": -VOLUME_MAX, "aria-valuemax": 0, "data-testid": testId }, fill, value);
      bindControl(element, { axis: "x", min: VOLUME_MAX, max: 0, up: -1, page: 6, reset: VOLUME_RESET, get, set, enabled });
      const show = (volume: number) => {
        fill.style.width = `${((VOLUME_MAX - Math.min(VOLUME_MAX, Math.max(0, volume))) / VOLUME_MAX) * 100}%`;
        value.textContent = formatVolume(volume);
        element.setAttribute("aria-valuenow", String(-volume));
        element.setAttribute("aria-valuetext", formatVolume(volume));
      };
      return { element, show };
    };

    const rows = CONTROL_ROOM_OUTPUTS.flatMap((id) => {
      const output = outputs.outputs[id];
      if (output === undefined) return [];
      const state = outputs.state(id);
      const volume = slider(`${output.name} volume`, `cr-volume-${id}`, () => state.peek().volume, (v) => outputs.setVolume(id, v));
      const mute = h("button", { type: "button", class: "mute", "data-testid": `cr-mute-${id}`, "aria-label": `${output.name} mute`, "on:click": () => outputs.setMute(id, !state.peek().mute) }, "Mute");
      const dim = output.dim ? h("button", { type: "button", class: "dim", "data-testid": `cr-dim-${id}`, "aria-label": `${output.name} dim`, "on:click": () => outputs.setDim(id, !state.peek().dim) }, "Dim") : undefined;
      // Mono is reported (Quadro) but has no command, so it is a badge; the switch below is the mix's.
      const mono = h("span", { class: "badge", title: `The device reports ${output.name} in mono`, hidden: true }, "MONO");
      this.watch(() => {
        const s = state.value;
        volume.show(s.volume);
        mute.setAttribute("aria-pressed", String(s.mute));
        dim?.setAttribute("aria-pressed", String(s.dim));
        mono.hidden = !s.mono;
      });
      return [h("div", { class: "group", "data-testid": `cr-output-${id}` }, h("div", { class: "head" }, h("span", { class: "label" }, output.name), mono, h("span", { class: "spacer" }), h("div", { class: "buttons" }, mute, dim)), volume.element)];
    });

    // Talkback: the Studio+ only. The Quadro's panel has no talkback commands, so it shows nothing of it.
    let talkback: HTMLElement | undefined;
    if (outputs.talkback !== undefined) {
      const talk = h("button", { type: "button", class: "talk", "data-testid": "cr-talk", "aria-label": "Talkback (hold to talk)", title: "Hold to talk" }, "Talk");
      bindMomentary(talk, (on) => outputs.setTalk(on), enabled);
      const level = slider("Talkback level", "cr-talk-volume", () => outputs.talk.peek().volume, (v) => outputs.setTalkbackVolume(v));
      const destinations = outputs.talkback.destinations.map((d) =>
        h("button", { type: "button", class: "to", "data-testid": `cr-talk-to-${d.id}`, "aria-label": `Talkback to ${d.name}`, "on:click": () => outputs.setTalkbackTo(d.id, !(outputs.talk.peek().to[d.id] ?? false)) }, d.name),
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

    // Mono (P57) for the mix chosen on the Mixer page. The pans it keeps to restore must be the
    // device's, so the mixes are read first if nothing has read them yet (a read is kept, P80).
    const mono = h("button", {
      type: "button",
      class: "mono",
      "data-testid": "cr-mono",
      "on:click": async () => {
        const mix = channels.meteredMix.peek();
        const on = !channels.isMono(mix);
        if (store.mixesToRead(deviceId)) await store.readMixes(deviceId);
        channels.setMono(mix, on);
      },
    }, "Mono");
    const mixCaption = h("span", { class: "caption", "data-testid": "cr-mono-mix" });
    this.watch(() => {
      const mix = channels.meteredMix.value;
      const name = channels.layout.value.mixes[mix]?.name;
      const label = `Mix ${mix + 1}`;
      mixCaption.textContent = name ? `${label}: ${name}` : label;
      mono.setAttribute("aria-pressed", String(channels.isMono(mix)));
      mono.setAttribute("aria-label", `${label} mono`);
      mono.title = `Sum ${label} to mono: pans its channels to centre, and restores them when turned off. The mix is the one chosen on the Mixer page.`;
    });

    this.root.replaceChildren(
      h("div", { class: "device", title: model }, model),
      ...rows,
      ...(talkback === undefined ? [] : [talkback]),
      h("div", { class: "group" }, h("div", { class: "head" }, mono, mixCaption)),
    );

    this.watch(() => {
      const connected = store.connected.value;
      for (const button of this.root.querySelectorAll("button")) button.disabled = !connected;
      for (const control of this.root.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!connected));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-control-room": GaControlRoom;
    "ga-monitor": GaMonitor;
  }
}
