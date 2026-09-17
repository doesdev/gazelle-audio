import { test } from "node:test";
import assert from "node:assert/strict";

import { ATTACK_MS, METER_FLOOR, MeterTicker, PEAK_FALL_DB_PER_S, PEAK_HOLD_MS, RELEASE_DB_PER_S, restingMeter, settled, stepMeter } from "../src/elements/meter-motion.ts";

/** Runs the ballistics for `ms` in frames of `frame` ms towards `target` (dB below full scale). */
function run(state: ReturnType<typeof restingMeter>, target: number, ms: number, frame = 16) {
  let s = state;
  for (let t = 0; t < ms; t += frame) s = stepMeter(s, target, frame);
  return s;
}

test("a meter rests at the floor, and a steady level settles there with nothing left to animate", () => {
  const rest = restingMeter();
  assert.deepEqual(rest, { level: METER_FLOOR, peak: METER_FLOOR, held: 0 });
  assert.equal(settled(rest, METER_FLOOR), true);
  assert.equal(settled(rest, 20), false, "a new level wakes it");
});

test("attack is quick but not a jump: most of the way within a few time constants, never past the signal", () => {
  const first = stepMeter(restingMeter(), 0, 5);
  assert.ok(first.level < METER_FLOOR && first.level > 0, "one short frame moves it part of the way, not all");
  const soon = run(restingMeter(), 6, ATTACK_MS * 5, 5);
  assert.ok(soon.level < 7, `within five time constants it is almost there (${soon.level})`);
  assert.ok(soon.level >= 6, "it never overshoots the signal");
});

test("release falls at a steady rate rather than dropping with each report, and stops at the signal", () => {
  const loud = run(restingMeter(), 0, 200);
  assert.ok(loud.level < 0.5);
  const later = run(loud, 50, 1000);
  assert.ok(Math.abs(later.level - RELEASE_DB_PER_S) < 1, `a second of silence lets it fall about ${RELEASE_DB_PER_S} dB (${later.level})`);
  const quiet = run(loud, 20, 10_000);
  assert.equal(quiet.level, 20, "it comes to rest at the signal, not below it");
});

test("the peak holds, then falls, and a louder signal takes it at once", () => {
  let s = run(restingMeter(), 3, 100);
  assert.equal(s.peak, 3, "the peak is the signal's own, not the smoothed bar's");
  s = run(s, 40, PEAK_HOLD_MS - 100);
  assert.equal(s.peak, 3, "held while the bar falls away");
  s = run(s, 40, 1000);
  assert.ok(s.peak > 3 + PEAK_FALL_DB_PER_S * 0.5, `after the hold it falls (${s.peak})`);
  s = stepMeter(s, 1, 16);
  assert.equal(s.peak, 1);
  assert.equal(s.held, PEAK_HOLD_MS);
  s = run(s, 60, 60_000);
  assert.deepEqual(s, restingMeter(), "given time, bar and peak come back to rest");
});

test("the ticker runs one frame loop for every meter, and sleeps when none needs a frame", () => {
  const frames: ((now: number) => void)[] = [];
  const ticker = new MeterTicker((callback) => frames.push(callback));
  let a = 0;
  let b = 0;
  let aWants = 2;
  ticker.wake(() => (a++, --aWants > 0));
  ticker.wake(() => (b++, false));
  assert.equal(frames.length, 1, "two meters, one frame request");
  frames.shift()?.(1000);
  assert.deepEqual([a, b], [1, 1]);
  assert.equal(frames.length, 1, "one meter still moving asks for another frame");
  frames.shift()?.(1016);
  assert.deepEqual([a, b], [2, 1], "a meter that settled is not called again");
  assert.equal(frames.length, 0, "with every meter settled the loop sleeps");
});
