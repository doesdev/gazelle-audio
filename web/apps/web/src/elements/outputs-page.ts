// <ga-outputs device-id="…">: a device's hardware outputs, each with the discrete controls its vendor
// panel binds: volume (dB of attenuation, 0 dB to -inf), mute and, on the Quadro, dim. Values come
// from the device's reports; changes send set_volume, set_mute and set_dim (OutputsModel).

import { h } from "../core/dom.ts";
import { formatVolume, TRIM_LABELS, VOLUME_MAX, type OutputInfo, type OutputsModel, type TrimInfo } from "../store/outputs.ts";
import { bindControl, bindMomentary, levelReset } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import type { ControlHost } from "./inputs-page.ts";

/** Where a double-click puts a volume: -20 dB, the safe level every level resets to (the user, 2026-09-18). */
const VOLUME_RESET = 20;

/** The styles of an output's row, for any element that shows one. */
export const OUTPUT_CONTROL_STYLES = `
  .output {
    display: grid;
    grid-template-columns: minmax(72px, 110px) minmax(0, 1fr) auto;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: 3px;
    background: var(--ga-surface-raised);
  }
  .name { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; }
  .volume {
    position: relative;
    height: 22px;
    border: 1px solid var(--ga-border-subtle);
    border-radius: 3px;
    background: var(--ga-surface-inset);
    cursor: ew-resize;
    touch-action: none;
    outline: none;
  }
  .volume:focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
  .volume .fill { position: absolute; top: 0; bottom: 0; left: 0; background: var(--ga-accent); opacity: 0.6; }
  .volume .value { position: absolute; inset: 0; font-size: 11px; line-height: 20px; text-align: center; font-variant-numeric: tabular-nums; pointer-events: none; }
  .volume[aria-disabled="true"] { cursor: not-allowed; opacity: 0.55; }
  .toggles { display: flex; gap: 4px; }
  .toggles button { min-width: 44px; font-size: 11px; font-weight: 700; }
  .mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
  .dim[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
  .name-cell { display: flex; align-items: center; gap: 6px; min-width: 0; }
  .mono { padding: 0 4px; border-radius: 2px; font-size: 9px; font-weight: 700; letter-spacing: 0.06em; color: var(--ga-text-inverse); background: var(--ga-accent); }
`;

export class GaOutputs extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 12px; }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .spacer { flex: 1; }
      .note { margin: 0; font-size: 11px; color: var(--ga-text-muted); }
      .last-sent { display: flex; min-width: 0; max-width: 100%; font-size: 11px; white-space: nowrap; }
      .last-sent code { min-width: 0; overflow: hidden; text-overflow: ellipsis; font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      .rows { display: grid; gap: 6px; max-width: 640px; }
      ${OUTPUT_CONTROL_STYLES}
      h2 { margin: 8px 0 6px; }
      .settings { display: grid; gap: 6px; max-width: 640px; }
      .setting { display: grid; grid-template-columns: minmax(72px, 110px) minmax(0, 1fr); align-items: center; gap: 10px; padding: 6px 8px; border-radius: 3px; background: var(--ga-surface-raised); }
      .setting select { justify-self: start; min-height: 24px; }
      .talk[aria-pressed="true"] { background: var(--ga-state-solo); color: var(--ga-text-inverse); }
      .destinations { display: flex; flex-wrap: wrap; gap: 4px; }
      .destinations button[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .note-inline { font-size: 11px; color: var(--ga-text-muted); }
      .hard-mute { font-size: 11px; font-weight: 700; }
      .in-cr[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
      .hard-mute[aria-pressed="true"] { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      @media (max-width: 480px) {
        .output { grid-template-columns: 1fr auto; }
        .volume { grid-column: 1 / -1; grid-row: 2; }
      }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    if (store.topology(deviceId) === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so its outputs are not known.`));
      return;
    }
    const outputs = store.outputs(deviceId);
    this.onDisconnect(outputs.activate());
    const enabled = () => store.connected.peek();

    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent" });
    // The Quadro's one switch over all four outputs; the vendor panel throws it while it restores a
    // session, and it is the same thing to reach for before changing monitors.
    const hardMute = !outputs.hasHardMute
      ? undefined
      : h("button", {
          type: "button",
          class: "hard-mute",
          "data-control": "",
          "data-testid": "hard-mute",
          "aria-label": "Hard mute: mute every output",
          title: "Mutes every output at once",
          "on:click": () => outputs.setHardMute(!outputs.hardMute.peek()),
        }, "Hard mute");
    const note = h("p", { class: "note" });
    const host = { watch: (fn: () => void) => this.watch(fn), onDisconnect: (fn: () => void) => this.onDisconnect(fn) };
    const rows = h("div", { class: "rows" }, outputs.outputs.map((output) => outputRow(host, outputs, output, enabled, this.#inControlRoom(deviceId, output))));

    const trims = h("section", {}, h("h2", {}, "Trims"), h("div", { class: "settings" }, outputs.trims.map((trim) => this.#trim(outputs, trim))));
    const talkback = outputs.talkback === undefined ? undefined : this.#talkback(outputs, enabled);
    this.root.replaceChildren(
      h("div", { class: "bar" }, ...(hardMute === undefined ? [] : [hardMute]), h("span", { class: "spacer" }), lastSent),
      note,
      rows,
      trims,
      ...(talkback === undefined ? [] : [talkback]),
    );

    if (hardMute !== undefined) {
      this.watch(() => {
        hardMute.setAttribute("aria-pressed", String(outputs.hardMute.value));
        hardMute.disabled = !store.connected.value;
      });
    }

    this.watch(() => {
      note.textContent = outputs.outputs.length > 0 && !outputs.state(0).value.known ? "The device has not reported its output levels yet, so controls start at defaults and send when changed." : "";
    });
    this.watch(() => {
      const sent = store.lastSent.value;
      if (sent === undefined || sent.deviceId !== deviceId) {
        lastSent.textContent = store.server.value.dry_run ? "Dry run: nothing is written to the device" : "";
        return;
      }
      lastSent.replaceChildren(`${sent.dryRun ? "Dry run, would send" : "Sent"} ${sent.command}: `, h("code", { title: sent.hex }, sent.hex));
    });
    this.watch(() => {
      const connected = store.connected.value;
      for (const button of this.root.querySelectorAll<HTMLButtonElement>("button[data-control]")) button.disabled = !connected;
      for (const control of this.root.querySelectorAll('[role="slider"]')) control.setAttribute("aria-disabled", String(!connected));
    });
  }

  /** Whether the output shows in the Control Room panel: the user's choice per device, kept in the workspace. */
  #inControlRoom(deviceId: string, output: OutputInfo): HTMLElement {
    const store = useStore();
    const shown = store.controlRoomOutputs(deviceId);
    // A toggle beside Mute and Dim, drawn as they are (the user, 2026-09-18).
    const button = h(
      "button",
      {
        type: "button",
        class: "in-cr",
        "data-testid": `out-in-cr-${output.id}`,
        "aria-label": `${output.name} in the Control Room`,
        title: `Show ${output.name} in the Control Room panel`,
        "on:click": () => store.setInControlRoom(deviceId, output.id, !shown.peek().includes(output.id)),
      },
      "CR",
    );
    this.watch(() => {
      button.setAttribute("aria-pressed", String(shown.value.includes(output.id)));
      button.disabled = !store.connected.value;
    });
    return button;
  }

  #trim(outputs: OutputsModel, trim: TrimInfo): HTMLElement {
    const state = outputs.trim(trim.id);
    const select = h(
      "select",
      { "aria-label": `${trim.name} trim`, "data-testid": `trim-${trim.id}`, "on:change": () => outputs.setTrim(trim.id, Number(select.value)) },
      TRIM_LABELS.map((label, index) => h("option", { value: String(index) }, label)),
    );
    this.watch(() => {
      // A report outside the seven steps (the loopback's test pattern) shows the nearest one, not a blank.
      select.value = String(Math.min(TRIM_LABELS.length - 1, Math.max(0, state.value.index)));
      select.disabled = !useStore().connected.value;
    });
    return h("div", { class: "setting" }, h("span", { class: "name" }, trim.name), select);
  }

  #talkback(outputs: OutputsModel, enabled: () => boolean): HTMLElement {
    // Talk is momentary: on only while held.
    const talk = h("button", { type: "button", class: "talk", "data-control": "", "data-testid": "talk", "aria-label": "Talkback (hold to talk)", title: "Hold to talk" }, "Talk");
    bindMomentary(talk, (on) => outputs.setTalk(on), enabled);
    const fill = h("div", { class: "fill" });
    const value = h("span", { class: "value" });
    // The panel's talkback control is a level fader, on the outputs' scale: 0 dB at the right, -inf at the left.
    const volume = h("div", { class: "volume", role: "slider", tabindex: 0, "aria-label": "Talkback level", "aria-valuemin": -VOLUME_MAX, "aria-valuemax": 0, "data-testid": "talk-volume" }, fill, value);
    bindControl(volume, { axis: "x", min: VOLUME_MAX, max: 0, up: -1, page: 6, reset: VOLUME_RESET, get: () => outputs.talk.peek().volume, set: (v) => outputs.setTalkbackVolume(v), enabled, level: levelReset(useStore().doubleClickUnity, (fn) => this.watch(fn)) });
    const destinations = (outputs.talkback?.destinations ?? []).map((d) =>
      h("button", { type: "button", "data-control": "", "data-testid": `talk-to-${d.id}`, "aria-label": `Talkback to ${d.name}`, "on:click": () => outputs.setTalkbackTo(d.id, !(outputs.talk.peek().to[d.id] ?? false)) }, d.name),
    );
    this.watch(() => {
      const t = outputs.talk.value;
      talk.setAttribute("aria-pressed", String(t.on));
      fill.style.width = `${((VOLUME_MAX - Math.min(VOLUME_MAX, Math.max(0, t.volume))) / VOLUME_MAX) * 100}%`;
      value.textContent = formatVolume(t.volume);
      volume.setAttribute("aria-valuenow", String(-t.volume));
      volume.setAttribute("aria-valuetext", formatVolume(t.volume));
      for (const [i, button] of destinations.entries()) button.setAttribute("aria-pressed", String(t.to[i] ?? false));
    });
    return h(
      "section",
      {},
      h("h2", {}, "Talkback"),
      h(
        "div",
        { class: "settings" },
        h("div", { class: "setting" }, h("span", { class: "name" }, "Talk"), h("div", { class: "toggles" }, talk)),
        h("div", { class: "setting" }, h("span", { class: "name" }, "Level"), volume),
        h("div", { class: "setting" }, h("span", { class: "name" }, "Send to"), h("div", { class: "destinations" }, destinations)),
      ),
    );
  }
}

/**
 * One output's row: its volume (dB of attenuation), mute, the Quadro's dim, and the mono the Quadro
 * reports. `extra` goes after the buttons (the Outputs page's Control Room choice).
 */
export function outputRow(host: ControlHost, outputs: OutputsModel, output: OutputInfo, enabled: () => boolean, extra?: HTMLElement): HTMLElement {
  const state = outputs.state(output.id);
  const fill = h("div", { class: "fill" });
  const value = h("span", { class: "value" });
  const volume = h(
    "div",
    { class: "volume", role: "slider", tabindex: 0, "aria-label": `${output.name} volume`, "aria-valuemin": -VOLUME_MAX, "aria-valuemax": 0, "data-testid": `out-volume-${output.id}` },
    fill,
    value,
  );
  bindControl(volume, { axis: "x", min: VOLUME_MAX, max: 0, up: -1, page: 6, reset: VOLUME_RESET, get: () => state.peek().volume, set: (v) => outputs.setVolume(output.id, v), enabled, level: levelReset(useStore().doubleClickUnity, (fn) => host.watch(fn)) });
  const mute = h("button", { type: "button", class: "mute", "data-control": "", "data-testid": `out-mute-${output.id}`, "aria-label": `${output.name} mute`, "on:click": () => outputs.setMute(output.id, !state.peek().mute) }, "Mute");
  const dim = output.dim ? h("button", { type: "button", class: "dim", "data-control": "", "data-testid": `out-dim-${output.id}`, "aria-label": `${output.name} dim`, "on:click": () => outputs.setDim(output.id, !state.peek().dim) }, "Dim") : undefined;
  // Mono is reported (Quadro) but has no command, so it is a badge, not a button.
  const mono = h("span", { class: "mono", "data-testid": `out-mono-${output.id}`, title: "The device reports this output in mono", hidden: true }, "MONO");

  host.watch(() => {
    const s = state.value;
    fill.style.width = `${((VOLUME_MAX - Math.min(VOLUME_MAX, Math.max(0, s.volume))) / VOLUME_MAX) * 100}%`;
    value.textContent = formatVolume(s.volume);
    volume.setAttribute("aria-valuenow", String(-s.volume));
    volume.setAttribute("aria-valuetext", formatVolume(s.volume));
    mute.setAttribute("aria-pressed", String(s.mute));
    dim?.setAttribute("aria-pressed", String(s.dim));
    mono.hidden = !s.mono;
  });

  return h("div", { class: "output", "data-testid": `output-${output.id}` }, h("span", { class: "name-cell" }, h("span", { class: "name" }, output.name), mono), volume, h("div", { class: "toggles" }, mute, dim, extra));
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-outputs": GaOutputs;
  }
}
