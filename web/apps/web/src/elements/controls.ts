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
  /**
   * The value at a fraction of the control's travel (0 at its top or left edge), for a control whose
   * scale is not linear, like a fader's audio taper. Linear from `min` to `max` when left out.
   */
  valueAt?(fraction: number): number;
}

/** Pointer drag, wheel, double-click reset and keyboard control of a value along one axis. */
export function bindControl(element: HTMLElement, options: ControlOptions): void {
  const valueAt = (event: PointerEvent) => {
    const rect = element.getBoundingClientRect();
    const fraction = Math.min(1, Math.max(0, options.axis === "y" ? (event.clientY - rect.top) / rect.height : (event.clientX - rect.left) / rect.width));
    return options.valueAt === undefined ? options.min + fraction * (options.max - options.min) : options.valueAt(fraction);
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

/**
 * A momentary button: `set(true)` while it is held (pointer, or Space/Enter), `set(false)` on
 * release, and on leaving, cancelling or losing focus, so it can never stay on by accident.
 * Used for talkback, which the user wants only while held.
 */
export function bindMomentary(button: HTMLElement, set: (on: boolean) => void, enabled: () => boolean): void {
  let held = false;
  const press = () => {
    if (held || !enabled()) return;
    held = true;
    set(true);
  };
  const release = () => {
    if (!held) return;
    held = false;
    set(false);
  };
  button.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    press();
  });
  for (const type of ["pointerup", "pointerleave", "pointercancel", "blur"]) button.addEventListener(type, release);
  button.addEventListener("keydown", (event) => {
    if ((event.key === " " || event.key === "Enter") && !event.repeat) {
      event.preventDefault();
      press();
    }
  });
  button.addEventListener("keyup", (event) => {
    if (event.key === " " || event.key === "Enter") release();
  });
}
