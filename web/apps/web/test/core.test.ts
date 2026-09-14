import { test } from "node:test";
import assert from "node:assert/strict";

import { frameWriter } from "../src/core/frame.ts";
import { batch, computed, effect, signal, untracked } from "../src/core/signal.ts";

test("a signal holds a value and ignores equal writes", () => {
  const level = signal(1);
  const runs: number[] = [];
  const dispose = effect(() => {
    runs.push(level.value);
  });
  level.value = 1;
  level.value = 2;
  assert.deepEqual(runs, [1, 2]);
  const loose = signal({ a: 1 }, (x, y) => x.a === y.a);
  loose.value = { a: 1 };
  assert.equal(loose.peek().a, 1);
  dispose();
});

test("a computed is lazy, cached, and recomputes only when a source changed", () => {
  const a = signal(2);
  let computations = 0;
  const doubled = computed(() => {
    computations++;
    return a.value * 2;
  });
  assert.equal(computations, 0, "nothing is computed until read");
  assert.equal(doubled.value, 4);
  assert.equal(doubled.value, 4);
  assert.equal(computations, 1, "a repeated read uses the cached value");
  a.value = 3;
  assert.equal(computations, 1, "a write only marks it stale");
  assert.equal(doubled.value, 6);
  assert.equal(computations, 2);
});

test("effects see a consistent graph and run once per change", () => {
  const a = signal(1);
  const b = computed(() => a.value * 2);
  const c = computed(() => a.value + b.value);
  const seen: number[] = [];
  const dispose = effect(() => {
    seen.push(c.value);
  });
  a.value = 2;
  assert.deepEqual(seen, [3, 6], "no glitch such as 2 + 2 = 4 is ever observed");
  dispose();
});

test("an effect does not rerun when a computed it reads keeps its value", () => {
  const n = signal(1);
  const odd = computed(() => n.value % 2 === 1);
  let runs = 0;
  const dispose = effect(() => {
    void odd.value;
    runs++;
  });
  n.value = 3;
  assert.equal(runs, 1);
  n.value = 4;
  assert.equal(runs, 2);
  dispose();
});

test("batch coalesces writes into one effect run", () => {
  const left = signal(0);
  const right = signal(0);
  const sums: number[] = [];
  const dispose = effect(() => {
    sums.push(left.value + right.value);
  });
  batch(() => {
    left.value = 1;
    right.value = 2;
  });
  assert.deepEqual(sums, [0, 3]);
  dispose();
});

test("dependencies follow what the last run actually read", () => {
  const useLeft = signal(true);
  const left = signal("L");
  const right = signal("R");
  const seen: string[] = [];
  const dispose = effect(() => {
    seen.push(useLeft.value ? left.value : right.value);
  });
  right.value = "R2";
  assert.deepEqual(seen, ["L"], "right is not a dependency yet");
  useLeft.value = false;
  left.value = "L2";
  assert.deepEqual(seen, ["L", "R2"], "left is no longer a dependency");
  dispose();
});

test("cleanups run before a rerun and on dispose; a disposed effect stops", () => {
  const n = signal(0);
  const log: string[] = [];
  const dispose = effect(() => {
    const value = n.value;
    log.push(`run ${value}`);
    return () => log.push(`cleanup ${value}`);
  });
  n.value = 1;
  dispose();
  n.value = 2;
  assert.deepEqual(log, ["run 0", "cleanup 0", "run 1", "cleanup 1"]);
});

test("untracked reads do not subscribe", () => {
  const tracked = signal(0);
  const ignored = signal(0);
  let runs = 0;
  const dispose = effect(() => {
    void tracked.value;
    untracked(() => ignored.value);
    runs++;
  });
  ignored.value = 1;
  assert.equal(runs, 1);
  tracked.value = 1;
  assert.equal(runs, 2);
  dispose();
});

test("an effect that keeps changing a signal it reads is stopped with an error", () => {
  const n = signal(0);
  // Dependencies are recorded when a run finishes, so the first run's own write reaches no
  // subscriber; the runaway loop starts with the next change.
  const dispose = effect(() => {
    n.value = n.value + 1;
  });
  assert.equal(n.peek(), 1);
  assert.throws(() => {
    n.value = 100;
  }, /did not settle/);
  dispose();
});

test("the frame writer applies the last write per signal once per frame, in one batch", () => {
  const frames: (() => void)[] = [];
  const writer = frameWriter((callback) => frames.push(callback));
  const meter = signal(0);
  const other = signal(0);
  const seen: string[] = [];
  const dispose = effect(() => {
    seen.push(`${meter.value}/${other.value}`);
  });

  writer.write(meter, 10);
  writer.write(meter, 20);
  writer.write(other, 5);
  assert.equal(frames.length, 1, "one frame is requested however many writes arrive");
  assert.equal(writer.pending, 2);
  assert.equal(meter.peek(), 0, "nothing is applied before the frame");

  frames.shift()?.();
  assert.deepEqual(seen, ["0/0", "20/5"], "the last value wins and the effect runs once");
  assert.equal(writer.pending, 0);

  writer.write(meter, 30);
  assert.equal(frames.length, 1, "the next write requests the next frame");
  dispose();
});
