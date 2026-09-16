import { test } from "node:test";
import assert from "node:assert/strict";

import { lobePath, polarLobes, polarResponse, stereoOrientation, type Lobe } from "../src/elements/polar-plot.ts";

const close = (a: number, b: number, eps = 1e-9) => Math.abs(a - b) < eps;
const deg = (d: number) => (d * Math.PI) / 180;
const distance = ([x, y]: readonly [number, number]) => Math.hypot(x, y);

/** The point of `lobes` in the direction `degrees` (clockwise from the front), away from the centre. */
function pointAt(lobes: readonly Lobe[], degrees: number): { lobe: Lobe; point: readonly [number, number] } | undefined {
  const [ux, uy] = [Math.sin(deg(degrees)), -Math.cos(deg(degrees))];
  for (const lobe of lobes) {
    for (const point of lobe.points) {
      const d = distance(point);
      if (d > 1e-6 && close(point[0] / d, ux, 1e-6) && close(point[1] / d, uy, 1e-6)) return { lobe, point };
    }
  }
  return undefined;
}

test("the response is 1 at the front for every pattern, and its landmarks are omni, cardioid and figure-8", () => {
  for (const angle of [1, 0.5, 0, -0.5, -1]) assert.ok(close(polarResponse(angle, 0), 1), `front of ${angle}`);
  for (const theta of [0, 45, 90, 135, 180, 270]) assert.ok(close(polarResponse(1, deg(theta)), 1), `omni at ${theta}°`);
  assert.ok(close(polarResponse(0, deg(90)), 0.5));
  assert.ok(close(polarResponse(0, deg(180)), 0));
  assert.ok(close(polarResponse(-1, deg(90)), 0));
  assert.ok(close(polarResponse(-1, deg(180)), -1));
  // Between omni and cardioid the rear is quieter but never silent; past cardioid it turns negative.
  assert.ok(close(polarResponse(0.5, deg(180)), 0.5));
  assert.ok(close(polarResponse(-0.5, deg(180)), -0.5));
});

test("omni is one circle, with nothing out of phase", () => {
  const lobes = polarLobes(1);
  assert.equal(lobes.length, 1);
  assert.equal(lobes[0]?.negative, false);
  for (const point of lobes[0]?.points ?? []) assert.ok(close(distance(point), 1), `radius ${distance(point)}`);
});

test("cardioid is one lobe with its null at the rear", () => {
  const lobes = polarLobes(0);
  assert.equal(lobes.length, 1);
  assert.equal(lobes[0]?.negative, false);
  assert.ok(close(distance(pointAt(lobes, 0)?.point ?? [0, 0]), 1));
  assert.ok(close(distance(pointAt(lobes, 90)?.point ?? [0, 0]), 0.5));
  // The rear sample sits on the centre, and the pattern closes into it from either side.
  assert.ok(lobes[0]?.points.some((p) => distance(p) < 1e-9));
  assert.equal(pointAt(lobes, 180), undefined);
  assert.ok(close(distance(pointAt(lobes, 150)?.point ?? [0, 0]), (1 + Math.cos(deg(150))) / 2));
  assert.ok(close(distance(pointAt(lobes, 210)?.point ?? [0, 0]), (1 + Math.cos(deg(150))) / 2));
});

test("figure-8 has its nulls at the sides and a rear lobe of inverted polarity as large as the front", () => {
  const lobes = polarLobes(-1);
  assert.equal(lobes.length, 2);
  const front = pointAt(lobes, 0);
  const rear = pointAt(lobes, 180);
  assert.equal(front?.lobe.negative, false);
  assert.equal(rear?.lobe.negative, true);
  assert.ok(close(distance(rear?.point ?? [0, 0]), 1));
  // Nothing at the sides but the centre: each lobe is closed through it.
  assert.equal(pointAt(lobes, 90), undefined);
  assert.equal(pointAt(lobes, 270), undefined);
  for (const lobe of lobes) {
    assert.ok(distance(lobe.points[0] ?? [1, 1]) < 1e-9, "starts at the centre");
    assert.ok(distance(lobe.points.at(-1) ?? [1, 1]) < 1e-9, "ends at the centre");
    // Each lobe stays on its own side: the front above the centre, the rear below.
    for (const [, y] of lobe.points) assert.ok(lobe.negative ? y >= -1e-9 : y <= 1e-9);
  }
});

test("past cardioid a small rear lobe turns out of phase, with nulls behind the sides", () => {
  const lobes = polarLobes(-0.5);
  assert.equal(lobes.length, 2);
  assert.ok(close(distance(pointAt(lobes, 180)?.point ?? [0, 0]), 0.5));
  assert.equal(pointAt(lobes, 180)?.lobe.negative, true);
  // cos θ = -(1 + a) / (1 - a): about 109.5°, so 100° is still in front and 120° already behind.
  assert.equal(pointAt(lobes, 100)?.lobe.negative, false);
  assert.equal(pointAt(lobes, 120)?.lobe.negative, true);
});

test("every pattern is symmetric about its front", () => {
  for (const angle of [1, 0.3, 0, -0.4, -1]) {
    for (const lobe of polarLobes(angle)) {
      for (const [x, y] of lobe.points) assert.ok(lobe.points.some(([mx, my]) => close(mx, -x, 1e-6) && close(my, y, 1e-6)), `${angle}: (${x}, ${y}) has no mirror`);
    }
  }
});

test("a rotation turns the whole pattern clockwise", () => {
  const turned = polarLobes(-1, 90);
  assert.equal(pointAt(turned, 90)?.lobe.negative, false);
  assert.equal(pointAt(turned, 270)?.lobe.negative, true);
  assert.equal(pointAt(turned, 0), undefined);
  assert.ok(close(distance(pointAt(polarLobes(0, -45), -45)?.point ?? [0, 0]), 1));
});

test("the stereo techniques turn the heads as they want them, and no technique turns nothing", () => {
  assert.deepEqual(stereoOrientation("XY", [0, 0]), [-45, 45]);
  assert.deepEqual(stereoOrientation("Blumlein", [-1, -1]), [-45, 45]);
  // M/S: the figure-8 is the side, across the cardioid mid, whichever head it is on.
  assert.deepEqual(stereoOrientation("M/S", [-1, 0]), [-90, 0]);
  assert.deepEqual(stereoOrientation("M/S", [0, -1]), [0, -90]);
  assert.deepEqual(stereoOrientation("None", [0, -1]), [0, 0]);
});

test("a lobe path is closed and scaled to the plot", () => {
  const path = lobePath([[0, 0], [0, -1], [1, 0]], 10, 12);
  assert.equal(path, "M12 12L12 2L22 12Z");
});
