// Meter ballistics, for the "plasma" look the user asked for (2026-09-16): the devices report peaks
// about 125 times a second, and drawn raw they flutter with every report. Here the bar rises almost
// at once, falls back at a steady rate, and a peak marker holds the loudest recent signal before it
// falls too. Levels are dB below full scale, as the report carries them, clamped to the meters'
// floor. `stepMeter` is the whole behaviour, as a pure function of time; `MeterTicker` runs every
// meter's animation on one frame loop, and sleeps when they have all come to rest.

import { effect, type ReadonlySignal } from "../core/signal.ts";

/** The quietest level a meter shows, in dB below full scale; anything quieter rests here. */
export const METER_FLOOR = 60;
/** How quickly the bar rises: the time constant of its approach to a louder signal. */
export const ATTACK_MS = 10;
/** How quickly the bar falls back when the signal drops. */
export const RELEASE_DB_PER_S = 12;
/** How long the peak marker holds the loudest recent signal before it falls. */
export const PEAK_HOLD_MS = 1500;
/** How quickly the peak marker falls once its hold is over. */
export const PEAK_FALL_DB_PER_S = 20;

/** What a meter shows: the bar and the peak marker in dB below full scale, and the peak's hold left. */
export interface MeterMotion {
  level: number;
  peak: number;
  held: number;
}

export function restingMeter(): MeterMotion {
  return { level: METER_FLOOR, peak: METER_FLOOR, held: 0 };
}

const clamp = (db: number) => Math.min(METER_FLOOR, Math.max(0, db));

/** One frame of `dt` ms towards `target` (dB below full scale). */
export function stepMeter(state: MeterMotion, target: number, dt: number): MeterMotion {
  const signal = clamp(target);
  let level = state.level;
  if (signal < level) level += (signal - level) * (1 - Math.exp(-dt / ATTACK_MS));
  else level = Math.min(signal, level + (RELEASE_DB_PER_S * dt) / 1000);

  let { peak, held } = state;
  if (signal <= peak && signal < METER_FLOOR) {
    // The signal's own peak, not the smoothed bar's: a marker at what was actually reached.
    peak = signal;
    held = PEAK_HOLD_MS;
  } else if (held > 0) {
    held = Math.max(0, held - dt);
  } else {
    // Falls towards the bar, never below it.
    peak = Math.min(level, peak + (PEAK_FALL_DB_PER_S * dt) / 1000);
  }
  return { level, peak, held };
}

/** Whether a meter showing `state` has nothing left to animate for `target`. */
export function settled(state: MeterMotion, target: number): boolean {
  return state.level === clamp(target) && state.peak === state.level && state.held <= 0;
}

/** One frame loop for every moving meter. A meter asks for frames until its callback returns false. */
export class MeterTicker {
  readonly #requestFrame: (callback: (now: number) => void) => void;
  readonly #running = new Set<(dt: number) => boolean>();
  #pending = false;
  #last: number | undefined;

  constructor(requestFrame: (callback: (now: number) => void) => void) {
    this.#requestFrame = requestFrame;
  }

  wake(frame: (dt: number) => boolean): void {
    this.#running.add(frame);
    this.#request();
  }

  #request(): void {
    if (this.#pending || this.#running.size === 0) return;
    this.#pending = true;
    this.#requestFrame((now) => {
      this.#pending = false;
      // A long gap (a hidden tab, the first frame) counts as one ordinary frame, so nothing leaps.
      const dt = this.#last === undefined || now - this.#last > 100 ? 16 : now - this.#last;
      this.#last = now;
      for (const frame of [...this.#running]) if (!frame(dt)) this.#running.delete(frame);
      if (this.#running.size === 0) this.#last = undefined;
      this.#request();
    });
  }
}

const ticker = new MeterTicker((callback) => (typeof requestAnimationFrame === "function" ? requestAnimationFrame(callback) : setTimeout(() => callback(performance.now()), 16)));

/**
 * Animates a meter from a level signal (dB below full scale, undefined for no reading) and draws
 * each frame with `draw`. Returns a disposer.
 */
export function animateMeter(level: ReadonlySignal<number | undefined>, draw: (motion: MeterMotion) => void): () => void {
  let state = restingMeter();
  let target = METER_FLOOR;
  let running = false;
  let disposed = false;
  draw(state);
  const frame = (dt: number) => {
    if (disposed) return (running = false);
    state = stepMeter(state, target, dt);
    draw(state);
    return (running = !settled(state, target));
  };
  const stop = effect(() => {
    target = level.value ?? METER_FLOOR;
    if (!running && !settled(state, target)) {
      running = true;
      ticker.wake(frame);
    }
  });
  return () => {
    disposed = true;
    stop();
  };
}
