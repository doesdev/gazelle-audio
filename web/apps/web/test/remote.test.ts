// The Remote page's store side and the touch rules it rests on, without a browser: a finger on a
// control (`bindControl` and its pure steps), a phone that never asks for what stays on the computer,
// the folded sections kept per browser, and a revoked phone stopping. The page itself is walked in
// e2e/remote.spec.ts.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { bindControl, TOUCH_RISE_GAP_MS, TOUCH_RISE_PER_S, TOUCH_SLOP_PX, touchMove, touchStart, type ControlOptions, type TouchScale } from "../src/elements/controls.ts";
import { REMOTE_SECTIONS_STORAGE_KEY, Store } from "../src/store/store.ts";
import { device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

// ---------------------------------------------------------------------------------------------
// The touch steps
// ---------------------------------------------------------------------------------------------

/** An output volume's scale: 96 (-inf) at the left, 0 dB at the right; up is a smaller number. */
const VOLUME: TouchScale = { valueAt: (f) => 96 - f * 96, positionOf: (v) => (96 - v) / 96, up: -1, rise: TOUCH_RISE_PER_S };

test("a finger that only lands, or wobbles inside the slop, moves nothing", () => {
  let drag = touchStart(1, 0.5, 200, 40, 0);
  for (const [px, at] of [[201, 16], [203, 32], [200 + TOUCH_SLOP_PX - 1, 48]] as const) {
    const step = touchMove(drag, px / 400, px, at, VOLUME);
    assert.equal(step.value, undefined, `${px} px is still a tap`);
    drag = step.drag;
  }
  assert.equal(drag.dragging, false);
});

test("a drag moves the value from where it was, not to where the finger is", () => {
  // The finger lands at the far right (0 dB there), with the volume at -40 dB.
  let drag = touchStart(1, 1, 400, 40, 0);
  // Leaving the slop picks the value up where the finger is, without a jump.
  let step = touchMove(drag, 0.98, 392, 16, VOLUME);
  assert.equal(step.value, undefined);
  drag = step.drag;
  // A quarter of the travel to the left: quieter by a quarter of the scale, at once.
  step = touchMove(drag, 0.73, 292, 32, VOLUME);
  assert.equal(Math.round(step.value ?? 0), 40 + 24);
});

test("louder is held back to a steady drag's pace, and a flick adds only a little", () => {
  let drag = touchStart(1, 0.1, 40, 60, 0);
  drag = touchMove(drag, 0.12, 48, 0, VOLUME).drag;
  // The whole travel to the right in one move, 16 ms later: at most 16 ms of rise.
  const flick = touchMove(drag, 1, 400, 16, VOLUME);
  assert.ok(flick.value !== undefined);
  assert.equal(Math.round((60 - flick.value) * 1000) / 1000, (TOUCH_RISE_PER_S * 16) / 1000);
  // A finger held still first gains nothing from the wait: the rise counts at most TOUCH_RISE_GAP_MS.
  const late = touchMove(drag, 1, 400, 5000, VOLUME);
  assert.equal(Math.round((60 - (late.value ?? 60)) * 1000) / 1000, (TOUCH_RISE_PER_S * TOUCH_RISE_GAP_MS) / 1000);
  // Having outrun it, moving back makes it quieter at once: the value was picked up at the finger.
  const back = touchMove(flick.drag, 0.9, 360, 32, VOLUME);
  assert.ok((back.value ?? 0) > (flick.value ?? 0), "back to the left is quieter");
});

test("quieter is never held back, and a control with no rise limit follows the finger", () => {
  let drag = touchStart(1, 0.9, 360, 10, 0);
  drag = touchMove(drag, 0.88, 352, 0, VOLUME).drag;
  const quieter = touchMove(drag, 0.08, 32, 16, VOLUME).value ?? 0;
  assert.ok(Math.abs(quieter - (10 + 0.8 * 96)) < 1e-9, `${quieter} followed the finger all the way`);
  const pan: TouchScale = { valueAt: (f) => f * 60, positionOf: (v) => v / 60, up: 1, rise: undefined };
  let panDrag = touchStart(1, 0.5, 200, 30, 0);
  panDrag = touchMove(panDrag, 0.52, 208, 0, pan).drag;
  assert.equal(touchMove(panDrag, 1.02, 408, 16, pan).value, 60);
});

// ---------------------------------------------------------------------------------------------
// bindControl, driven with pointer events
// ---------------------------------------------------------------------------------------------

/** Just enough of an element for `bindControl`: a 400 by 40 box at the origin, and events. */
class FakeElement extends EventTarget {
  title = "";
  readonly style: Record<string, string> = {};
  readonly #captured = new Set<number>();
  getBoundingClientRect() {
    return { left: 0, top: 0, width: 400, height: 40, right: 400, bottom: 40 };
  }
  focus(): void {}
  setPointerCapture(id: number): void {
    this.#captured.add(id);
  }
  hasPointerCapture(id: number): boolean {
    return this.#captured.has(id);
  }
}

function pointer(type: string, pointerType: "mouse" | "touch", clientX: number, timeStamp: number, extra: Record<string, unknown> = {}): Event {
  const event = new Event(type, { cancelable: true });
  Object.defineProperty(event, "timeStamp", { value: timeStamp });
  return Object.assign(event, { pointerType, pointerId: pointerType === "touch" ? 7 : 1, button: 0, clientX, clientY: 20, ctrlKey: false, metaKey: false, ...extra });
}

/** An output volume on a fake element, recording every value set. */
function volume(start = 40) {
  const element = new FakeElement();
  let value = start;
  const sets: number[] = [];
  const options: ControlOptions = {
    axis: "x",
    min: 96,
    max: 0,
    up: -1,
    page: 6,
    reset: 30,
    get: () => value,
    set: (v) => {
      value = Math.round(v);
      sets.push(value);
    },
    enabled: () => true,
    level: { unity: 0, resetText: "-30 dB", unityText: "0 dB", unityOnDoubleClick: () => false, watch: (fn) => fn() },
  };
  bindControl(element as unknown as HTMLElement, options);
  return { element, sets, value: () => value };
}

test("a tap on a fader does not jump the level", () => {
  const { element, sets } = volume(40);
  element.dispatchEvent(pointer("pointerdown", "touch", 390, 0));
  element.dispatchEvent(pointer("pointerup", "touch", 390, 80));
  element.dispatchEvent(new Event("click"));
  assert.deepEqual(sets, []);
  // Nor does the double tap a second one makes: a double-click from a finger is not a reset.
  element.dispatchEvent(pointer("pointerdown", "touch", 390, 120));
  element.dispatchEvent(pointer("pointerup", "touch", 390, 160));
  element.dispatchEvent(new Event("dblclick"));
  assert.deepEqual(sets, []);
});

test("a drag by touch moves the level, from where it was", () => {
  const { element, sets, value } = volume(40);
  element.dispatchEvent(pointer("pointerdown", "touch", 390, 0));
  // Leftwards, which is quieter and never held back.
  for (const [x, at] of [[380, 16], [300, 32], [200, 48]] as const) element.dispatchEvent(pointer("pointermove", "touch", x, at));
  element.dispatchEvent(pointer("pointerup", "touch", 200, 64));
  assert.ok(sets.length > 0, "the drag set the level");
  // 180 px of the 400 px travel from where the slop was left: about 43 dB quieter, not the -inf the finger's position would say.
  assert.ok(value() > 40 && value() < 96, `${value()} followed the drag`);
  assert.equal(value(), 40 + Math.round((180 / 400) * 96));
});

test("a mouse click still jumps to where it presses, and a double-click resets", () => {
  const { element, sets, value } = volume(40);
  element.dispatchEvent(pointer("pointerdown", "mouse", 400, 0));
  assert.equal(value(), 0, "a click at the right end is 0 dB, as at a desk");
  element.dispatchEvent(pointer("pointerup", "mouse", 400, 16));
  element.dispatchEvent(pointer("dblclick", "mouse", 400, 32));
  assert.equal(value(), 30);
  assert.deepEqual(sets, [0, 30]);
});

test("the browser keeps the other axis for scrolling", () => {
  assert.equal(volume().element.style["touchAction"], "pan-y");
});

// ---------------------------------------------------------------------------------------------
// The store on a phone
// ---------------------------------------------------------------------------------------------

function setup(phone: boolean, storage = new MemoryStorage()) {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  client.server = { ...client.server, phone };
  client.updateStatus = { version: "1.4.1", target: "x86_64-pc-windows-msvc", channel: "stable", check: true, auto_download: true, can_verify: true, last_check_ms: null, state: { state: "up_to_date" } };
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage, requestFrame: () => {}, themeSources: [] });
  return { client, timers, store, storage };
}

test("a phone never asks for update, aggregate or phone management: it is refused before any request", async () => {
  const { client, store, timers } = setup(true);
  assert.equal(store.phone, true);
  await store.start();
  timers.advance(120_000);
  await flush();
  assert.deepEqual(client.updateCalls, [], "no update status on load, and none polled");

  const phones = store.phones.follow();
  const aggregate = store.aggregate.activate();
  await flush();
  assert.deepEqual(client.remoteCalls, []);
  assert.deepEqual(client.aggregateCalls, []);
  assert.equal(store.phones.state.peek().state, "elsewhere", "the Phones section says where phones are managed");
  assert.equal(store.aggregate.offered.peek(), false, "the Aggregate page says it is not offered here");
  phones();
  aggregate();
  await store.checkForUpdate();
  assert.deepEqual(client.updateCalls, []);
  await store.close();
});

test("the computer itself still asks", async () => {
  const { client, store } = setup(false);
  assert.equal(store.phone, false);
  await store.start();
  await flush();
  assert.deepEqual(client.updateCalls, ["status"]);
  await store.close();
});

test("the Remote page's folded sections are kept per browser, and a bad entry is dropped", async () => {
  const storage = new MemoryStorage();
  const first = setup(true, storage).store;
  assert.equal(first.remoteSectionCollapsed("inputs"), false, "open until folded");
  first.setRemoteSectionCollapsed("inputs", true);
  assert.equal(first.remoteSectionCollapsed("inputs"), true);
  await first.close();

  const again = setup(true, storage).store;
  assert.equal(again.remoteSectionCollapsed("inputs"), true);
  assert.equal(again.remoteSectionCollapsed("mix"), false);
  await again.close();

  storage.setItem(REMOTE_SECTIONS_STORAGE_KEY, JSON.stringify({ mix: "yes", inputs: true }));
  const cleaned = setup(true, storage).store;
  assert.equal(cleaned.remoteSectionCollapsed("mix"), false);
  assert.equal(cleaned.remoteSectionCollapsed("inputs"), true);
  await cleaned.close();
});

test("a revoked phone: the store says so and stops asking the updater", async () => {
  const { client, store } = setup(false);
  await store.start();
  await flush();
  assert.equal(store.unpaired.peek(), false);
  client.emit("unpaired", "this phone is no longer paired");
  assert.equal(store.unpaired.peek(), true);
  await store.close();
});
