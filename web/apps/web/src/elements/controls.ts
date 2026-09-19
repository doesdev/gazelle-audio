// Value controls shared by the mixer strips and the inputs page, and the confirms for the large
// one-click actions.

import { h } from "../core/dom.ts";

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
  /**
   * Pixels at each end of the element outside the control's travel. A fader's cap is centred on its
   * value, so its travel runs from half a cap below the top to half a cap above the bottom.
   */
  inset?: number;
  /** For a level (a fader, a volume, a send, a return): unity by Ctrl+click, and `reset` is its safe level. */
  level?: LevelReset;
}

/**
 * A level's resets (the user, 2026-09-18): double-click goes to `reset`, a safe level near
 * -20 dB, and Ctrl+click (Cmd+click on a Mac keyboard, as 48V takes) to unity. A setting kept per
 * browser makes double-click unity too. The control's title says which does what.
 */
export interface LevelReset {
  /** 0 dB, or full for a return. */
  unity: number;
  /** How `reset` and `unity` read in the title: "-20 dB" and "0 dB". */
  resetText: string;
  unityText: string;
  /** The setting: true sends double-click to unity. Read at each double-click, and watched for the title. */
  unityOnDoubleClick(): boolean;
  /** The element's `watch`, so the title follows the setting. */
  watch(fn: () => void): void;
}

/** A level whose safe reset is -20 dB and whose unity is 0 dB, unless it says otherwise. */
export function levelReset(setting: { readonly value: boolean }, watch: (fn: () => void) => void, unity = 0, resetText = "-20 dB", unityText = "0 dB"): LevelReset {
  return { unity, resetText, unityText, unityOnDoubleClick: () => setting.value, watch };
}

/** What a level's title says its double-click and Ctrl+click do. */
export function levelTitle(level: LevelReset): string {
  return level.unityOnDoubleClick() ? `Double-click or Ctrl/Cmd+click: ${level.unityText}.` : `Double-click: ${level.resetText}. Ctrl/Cmd+click: ${level.unityText}.`;
}

/** Pointer drag, wheel, double-click reset and keyboard control of a value along one axis. */
export function bindControl(element: HTMLElement, options: ControlOptions): void {
  const valueAt = (event: PointerEvent) => {
    const rect = element.getBoundingClientRect();
    const inset = options.inset ?? 0;
    const fraction = Math.min(1, Math.max(0, options.axis === "y" ? (event.clientY - rect.top - inset) / (rect.height - 2 * inset) : (event.clientX - rect.left - inset) / (rect.width - 2 * inset)));
    return options.valueAt === undefined ? options.min + fraction * (options.max - options.min) : options.valueAt(fraction);
  };
  const level = options.level;
  level?.watch(() => {
    element.title = levelTitle(level);
  });
  element.addEventListener("pointerdown", (event) => {
    if (!options.enabled() || event.button !== 0) return;
    element.focus();
    event.preventDefault();
    // Ctrl+click on a level is unity, wherever it lands, and starts no drag.
    if (level !== undefined && (event.ctrlKey || event.metaKey)) return options.set(level.unity);
    element.setPointerCapture(event.pointerId);
    options.set(valueAt(event));
  });
  element.addEventListener("pointermove", (event) => {
    if (element.hasPointerCapture(event.pointerId)) options.set(valueAt(event));
  });
  element.addEventListener("dblclick", (event) => {
    if (!options.enabled()) return;
    // Two Ctrl+clicks have already set unity; the double-click they make is not a reset.
    if (level !== undefined && (event.ctrlKey || event.metaKey)) return;
    options.set(level?.unityOnDoubleClick() === true ? level.unity : options.reset);
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

/** How long a first click waits for its confirming second, as 48V's does. */
export const CONFIRM_MS = 3000;

/**
 * The two-click confirm 48V has, for a button whose click does something large (the user,
 * 2026-09-18): while `needed()` says so, the first click arms it, so it reads "Confirm" and is
 * outlined (`data-armed`), and a second click within `CONFIRM_MS` acts; a wait forgets it. When
 * `needed()` is false, turning a thing off, one click acts. Keys press it as they press any button,
 * and an `aria-label` on it keeps the name a screen reader hears, as 48V's does. Returns the disarm,
 * for a page that goes or a connection that drops. `idle` may be a function, for a label that
 * follows what the button controls (the driver's Safe Mode reads On or Off).
 */
export function bindConfirm(button: HTMLElement, idle: string | (() => string), act: () => void, needed: () => boolean = () => true): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const disarm = () => {
    clearTimeout(timer);
    timer = undefined;
    button.removeAttribute("data-armed");
    button.textContent = typeof idle === "string" ? idle : idle();
  };
  button.addEventListener("click", () => {
    if (timer !== undefined || !needed()) {
      disarm();
      act();
      return;
    }
    button.setAttribute("data-armed", "");
    button.textContent = "Confirm";
    timer = setTimeout(disarm, CONFIRM_MS);
  });
  return disarm;
}

/**
 * A menu whose choice is sent only from a Confirm button beside it: the clock source's and the
 * sample rate's (the user, 2026-09-18), and the driver's buffer size. Choosing another value shows
 * the button, outlined as an armed 48V is, and pressing it sends; a wait of `CONFIRM_MS` takes it
 * away and puts the menu back to `current`, what the device or driver has, and so does choosing
 * that value again. `explain` is the Confirm button's key for the explain mode.
 */
export function confirmedChoice(select: HTMLSelectElement, testId: string, explain: string, describe: (index: number) => string, send: (index: number) => void) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const confirm = h("button", { type: "button", class: "confirm", "data-testid": testId, "data-explain": explain, hidden: true }, "Confirm");
  const choice = {
    current: Number(select.value),
    confirm,
    armed: () => timer !== undefined,
    disarm: () => {
      clearTimeout(timer);
      timer = undefined;
      confirm.hidden = true;
      select.value = String(choice.current);
    },
  };
  select.addEventListener("change", () => {
    const index = Number(select.value);
    if (index === choice.current) return choice.disarm();
    clearTimeout(timer);
    confirm.title = describe(index);
    confirm.hidden = false;
    timer = setTimeout(choice.disarm, CONFIRM_MS);
  });
  confirm.addEventListener("click", () => {
    if (timer === undefined) return;
    choice.current = Number(select.value);
    choice.disarm();
    send(choice.current);
  });
  return choice;
}

/**
 * A menu that acts at once, unless the choice deserves a word first: `guard` returns the reason,
 * and the change then waits behind a Confirm button beside the menu, outlined as an armed 48V is,
 * with the reason on it. Pressing Confirm acts; choosing again asks again; a wait of `CONFIRM_MS`
 * puts the menu back to what was last acted on. So nothing changes about a menu whose choices are
 * all plain, and a choice that is not is never made by accident (the user, 2026-09-19: the mixer
 * let one input into one mix twice with no warning at all).
 *
 * `explain` is the Confirm button's key for the explain mode; `sync` puts the menu back where the
 * page rebuilds it from the outside, so what was last acted on stays the value to fall back to.
 */
export function guardedChoice(select: HTMLSelectElement, testId: string, explain: string, guard: (value: string) => string | undefined, act: (value: string) => void) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const confirm = h("button", { type: "button", class: "confirm", "data-testid": testId, "data-explain": explain, hidden: true }, "Confirm");
  const choice = {
    /** The value last acted on, which an unanswered warning goes back to. */
    current: select.value,
    confirm,
    armed: () => timer !== undefined,
    disarm: () => {
      clearTimeout(timer);
      timer = undefined;
      confirm.hidden = true;
      select.value = choice.current;
    },
    /** Follows the menu when the page rebuilds its options from the layout. */
    sync: (value: string) => {
      if (timer !== undefined) return;
      choice.current = value;
      select.value = value;
    },
  };
  const settle = (value: string) => {
    clearTimeout(timer);
    timer = undefined;
    confirm.hidden = true;
    choice.current = value;
    act(value);
  };
  select.addEventListener("change", () => {
    clearTimeout(timer);
    timer = undefined;
    confirm.hidden = true;
    const reason = guard(select.value);
    if (reason === undefined) return settle(select.value);
    confirm.title = reason;
    confirm.setAttribute("aria-label", `${reason}. Select again to do it anyway`);
    confirm.hidden = false;
    timer = setTimeout(choice.disarm, CONFIRM_MS);
  });
  confirm.addEventListener("click", () => {
    if (timer === undefined) return;
    settle(select.value);
  });
  return choice;
}

/**
 * A momentary button:`set(true)` while it is held (pointer, or Space/Enter), `set(false)` on
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
