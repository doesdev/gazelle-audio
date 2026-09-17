// <ga-output-meters>: the right zone's Meter panel. It meters the outputs of the device on the
// current page, or the one last selected (P71): a left/right pair of bars per output, on the same
// scale as the mixer's meters. The Quadro reports Monitor, HP1, HP2 and Line out in fixed fields,
// which were checked against what the user heard (hardware, 2026-09-16). The Studio+ reports its
// output levels only through a selectable meter bank, so it shows a note instead.

import { h } from "../core/dom.ts";
import { untracked } from "../core/signal.ts";
import { animateMeter, METER_FLOOR, type MeterMotion } from "./meter-motion.ts";
import { meterDeflection } from "../store/mixer.ts";
import { displayName } from "../store/store.ts";
import { meterGradient } from "../themes/theme.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { route } from "./router.ts";

const STATUS_REPORT = "0x73";

export class GaOutputMeters extends GaElement {
  static override styles = [
    sheet(`
      :host { display: grid; gap: 6px; }
      .device { font-size: 11px; color: var(--ga-text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .output { display: grid; grid-template-columns: minmax(48px, auto) minmax(0, 1fr) 40px; align-items: center; gap: 6px; }
      .output-name { font-size: 11px; color: var(--ga-text-secondary); }
      .bars { display: grid; gap: 2px; }
      .bar { position: relative; height: 5px; overflow: hidden; border-radius: 1px; background: var(--ga-meter-background); }
      .bar .gradient { position: absolute; inset: 0; background: var(--output-meter-gradient, var(--ga-accent)); }
      .bar .mask { position: absolute; top: 0; bottom: 0; right: 0; width: 100%; background: var(--ga-meter-background); }
      /* Plasma style, as the mixer's meters: a glow at the bar's leading edge and a held peak marker. */
      .bar .mask::after { content: ""; position: absolute; top: 0; bottom: 0; left: -2px; width: 2px; background: var(--ga-text-primary); opacity: 0.35; filter: blur(1.5px); }
      .bar .peak-mark { position: absolute; top: 0; bottom: 0; width: 2px; margin-left: -1px; background: var(--ga-text-primary); opacity: 0.85; box-shadow: 0 0 4px var(--ga-text-primary); }
      .bar .peak-mark[hidden] { display: none; }
      .peak { font-size: 10px; text-align: right; color: var(--ga-text-muted); font-variant-numeric: tabular-nums; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    let shown: string | undefined;
    // What the shown device's meters hold on to: its report and each bar's effect. Released when
    // another device is shown, so a panel switched back and forth does not pile them up.
    let held: (() => void)[] = [];
    const release = () => {
      for (const dispose of held.splice(0)) dispose();
    };
    this.onDisconnect(release);
    this.watch(() => {
      const current = route.value;
      const known = store.devices.value.filter((d) => d.family !== null);
      const id = known.find((d) => d.id === current.id)?.id ?? store.deviceInView(true);
      if (id === shown && this.root.childElementCount > 0) return;
      shown = id;
      release();
      const device = known.find((d) => d.id === id);
      if (device === undefined) {
        this.root.replaceChildren(h("p", { class: "placeholder" }, "No device of known model is connected."));
        return;
      }
      const meters = store.outputMeters(device.id);
      const name = h("span", { class: "device" }, displayName(device, store.workspace.peek()));
      if (meters === undefined) {
        this.root.replaceChildren(name, h("p", { class: "placeholder" }, "This model reports no output meters of its own: its output levels only reach its selectable meter bank."));
        return;
      }
      held.push(store.watchReport(device.id, STATUS_REPORT));
      // Untracked: the bars' own effects follow the meters; this watch follows only the device.
      untracked(() => this.root.replaceChildren(
        name,
        ...meters.map((meter) => {
          const peak = h("span", { class: "peak" });
          const peaks: Record<"left" | "right", number> = { left: METER_FLOOR, right: METER_FLOOR };
          const bar = (side: "left" | "right") => {
            const mask = h("div", { class: "mask" });
            const peakMark = h("div", { class: "peak-mark", hidden: "" });
            held.push(
              animateMeter(meter[side], (motion: MeterMotion) => {
                mask.style.width = `${100 - meterDeflection(motion.level)}%`;
                peakMark.hidden = motion.peak >= METER_FLOOR;
                peakMark.style.left = `${meterDeflection(motion.peak)}%`;
                peaks[side] = motion.peak;
                const loudest = Math.min(peaks.left, peaks.right);
                peak.textContent = loudest >= METER_FLOOR ? "< -60" : loudest < 0.5 ? "0" : `-${Math.round(loudest)}`;
              }),
            );
            return h("div", { class: "bar", "aria-hidden": "true" }, h("div", { class: "gradient" }), mask, peakMark);
          };
          return h("div", { class: "output", "data-output": meter.name, title: `${meter.name}: peak, dB below full scale` }, h("span", { class: "output-name" }, meter.name), h("div", { class: "bars" }, bar("left"), bar("right")), peak);
        }),
      ));
    });
    this.watch(() => {
      this.style.setProperty("--output-meter-gradient", meterGradient(store.theme.value.meter.gradient, undefined, "to right", (db) => meterDeflection(-db)));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-output-meters": GaOutputMeters;
  }
}
