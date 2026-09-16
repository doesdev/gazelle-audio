// The wheel on a select (feedback item 6): how wheel travel becomes option steps, and which
// option a step lands on. The listener that applies them runs in the browser (e2e/wheel.spec.ts).

import { test } from "node:test";
import assert from "node:assert/strict";

import { stepIndex, WHEEL_IDLE_MS, wheelStep, type WheelLatch } from "../src/elements/select-wheel.ts";

const PIXEL = 0;
const LINE = 1;
const PAGE = 2;

/** Runs wheel events through `wheelStep` in order, returning each result's step and whether it was taken. */
function run(events: { target?: object; deltaY: number; deltaMode?: number; timeStamp: number }[]): { step: number; take: boolean }[] {
  let latch: WheelLatch | undefined;
  return events.map(({ target, deltaY, deltaMode = PIXEL, timeStamp }) => {
    const result = wheelStep(latch, target, { deltaY, deltaMode, timeStamp });
    latch = result.latch;
    return { step: result.step, take: result.take };
  });
}

const select = {};

test("a mouse notch steps once, down to the next option and up to the previous", () => {
  assert.deepEqual(run([{ target: select, deltaY: 100, timeStamp: 0 }]), [{ step: 1, take: true }]);
  assert.deepEqual(run([{ target: select, deltaY: -100, timeStamp: 0 }]), [{ step: -1, take: true }]);
  // A larger notch (display scaling, or two notches in one event) is still one step, and leaves
  // nothing over to add a surprise step to the next notch.
  assert.deepEqual(
    run([120, 120, 120, 120, 120, 250].map((deltaY, i) => ({ target: select, deltaY, timeStamp: i * 500 }))).map((r) => r.step),
    [1, 1, 1, 1, 1, 1],
  );
});

test("a trackpad's small deltas add up to one step per notch's worth of travel", () => {
  const swipe = Array.from({ length: 20 }, (_, i) => ({ target: select, deltaY: 12, timeStamp: i * 16 }));
  const results = run(swipe);
  assert.deepEqual(
    results.map((r) => r.step),
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0],
  );
  // Every event over the select is taken, stepping or not, so the page does not scroll meanwhile.
  assert.ok(results.every((r) => r.take));
});

test("lines and pages have their own notch", () => {
  assert.deepEqual(run([{ target: select, deltaY: 3, deltaMode: LINE, timeStamp: 0 }]).map((r) => r.step), [1]);
  assert.deepEqual(run([1, 1, 1].map((deltaY, i) => ({ target: select, deltaY, deltaMode: LINE, timeStamp: i * 16 }))).map((r) => r.step), [0, 0, 1]);
  assert.deepEqual(run([{ target: select, deltaY: -1, deltaMode: PAGE, timeStamp: 0 }]).map((r) => r.step), [-1]);
});

test("reversing, or pausing, starts the travel again", () => {
  assert.deepEqual(
    run([
      { target: select, deltaY: 60, timeStamp: 0 },
      { target: select, deltaY: -60, timeStamp: 16 },
      { target: select, deltaY: -40, timeStamp: 32 },
    ]).map((r) => r.step),
    [0, 0, -1],
  );
  assert.deepEqual(
    run([
      { target: select, deltaY: 60, timeStamp: 0 },
      { target: select, deltaY: 60, timeStamp: WHEEL_IDLE_MS + 1 },
    ]).map((r) => r.step),
    [0, 0],
  );
});

test("a page scroll that brings a select under the pointer keeps scrolling the page", () => {
  assert.deepEqual(
    run([
      { deltaY: 100, timeStamp: 0 },
      { target: select, deltaY: 100, timeStamp: 100 },
      { target: select, deltaY: 100, timeStamp: 200 },
      // Once the wheel has rested, the select takes it.
      { target: select, deltaY: 100, timeStamp: 200 + WHEEL_IDLE_MS + 1 },
    ]),
    [
      { step: 0, take: false },
      { step: 0, take: false },
      { step: 0, take: false },
      { step: 1, take: true },
    ],
  );
});

test("a select rebuilt under the pointer by the change goes on taking the wheel", () => {
  assert.deepEqual(
    run([
      { target: select, deltaY: 100, timeStamp: 0 },
      { target: {}, deltaY: 100, timeStamp: 50 },
    ]),
    [
      { step: 1, take: true },
      { step: 1, take: true },
    ],
  );
});

test("a sideways wheel is left to the page", () => {
  assert.deepEqual(run([{ target: select, deltaY: 0, timeStamp: 0 }]), [{ step: 0, take: false }]);
});

test("a step lands on the next option it may, and nowhere past the ends", () => {
  const all = [true, true, true, true];
  assert.equal(stepIndex(all, 1, 1), 2);
  assert.equal(stepIndex(all, 1, -1), 0);
  assert.equal(stepIndex(all, 3, 1), undefined);
  assert.equal(stepIndex(all, 0, -1), undefined);
  // Disabled or hidden options are passed over, and a run of them at the end is an end.
  assert.equal(stepIndex([true, false, false, true], 0, 1), 3);
  assert.equal(stepIndex([true, false, false, true], 3, -1), 0);
  assert.equal(stepIndex([true, true, false, false], 1, 1), undefined);
  // With nothing selected, down picks the first option it may.
  assert.equal(stepIndex([false, true, true], -1, 1), 1);
  assert.equal(stepIndex([], -1, 1), undefined);
});
