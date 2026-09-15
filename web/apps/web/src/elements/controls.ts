// Value controls shared by the mixer strips and the inputs page.

export interface ControlOptions {
  axis: "x" | "y";
  /** Read at each event, so a caller may change the range (a preamp's gain range follows its type). */
  min: number;
  max: number;
  /** +1 when a larger value is "more" (pan right, send up); −1 for attenuation, where up means a smaller value. */
  up: 1 | -1;
  page: number;
  reset: number;
  get(): number;
  set(value: number): void;
  enabled(): boolean;
}

/** Pointer drag, wheel, double-click reset and keyboard control of a value along one axis. */
export function bindControl(element: HTMLElement, options: ControlOptions): void {
  const valueAt = (event: PointerEvent) => {
    const rect = element.getBoundingClientRect();
    const fraction = options.axis === "y" ? (event.clientY - rect.top) / rect.height : (event.clientX - rect.left) / rect.width;
    return options.min + Math.min(1, Math.max(0, fraction)) * (options.max - options.min);
  };
  element.addEventListener("pointerdown", (event) => {
    if (!options.enabled() || event.button !== 0) return;
    element.setPointerCapture(event.pointerId);
    element.focus();
    options.set(valueAt(event));
    event.preventDefault();
  });
  element.addEventListener("pointermove", (event) => {
    if (element.hasPointerCapture(event.pointerId)) options.set(valueAt(event));
  });
  element.addEventListener("dblclick", () => {
    if (options.enabled()) options.set(options.reset);
  });
  element.addEventListener(
    "wheel",
    (event) => {
      if (!options.enabled()) return;
      event.preventDefault();
      options.set(options.get() + (event.deltaY < 0 ? options.up : -options.up));
    },
    { passive: false },
  );
  element.addEventListener("keydown", (event) => {
    if (!options.enabled()) return;
    const steps: Record<string, number> = { ArrowUp: options.up, ArrowRight: options.up, ArrowDown: -options.up, ArrowLeft: -options.up, PageUp: options.page * options.up, PageDown: -options.page * options.up };
    if (event.key in steps) options.set(options.get() + (steps[event.key] ?? 0));
    else if (event.key === "Home") options.set(options.min);
    else if (event.key === "End") options.set(options.max);
    else return;
    event.preventDefault();
  });
}
