// The wheel steps a select, everywhere in the app: one listener on the document
// finds the select under the pointer through the event's composed path, so selects in any shadow
// root, and ones built later, need nothing of their own. A step picks the previous or next option
// that may be picked and fires `input` and `change` as a pick from the list does, so each page's
// handlers run unchanged. `data-no-wheel` on a select leaves it to the page, and on an option
// leaves it out of the stepping (an action such as "New group…").

/** How long the wheel must rest before a new gesture starts: travel resets, and so does a page scroll's hold. */
export const WHEEL_IDLE_MS = 400;

/**
 * Wheel travel that makes one step, per `deltaMode`: a mouse notch as Chromium reports it on
 * Windows (100 px), Firefox's three lines, one page.
 */
const NOTCH = [100, 3, 1] as const;

export interface WheelLatch {
  /** The gesture is scrolling the page: a select that comes under the pointer mid-gesture is left alone. */
  page: boolean;
  target: object | undefined;
  time: number;
  /** Travel towards the next step, in notches; its sign is the direction. */
  travel: number;
}

export interface WheelInput {
  deltaY: number;
  deltaMode: number;
  timeStamp: number;
}

/**
 * Folds one wheel event into the gesture so far. `target` is the select under the pointer, or
 * `undefined` when there is none (or it is not taking the wheel). `take` says the event belongs
 * to the select, so the page should not scroll; `step` is +1 for the next option (wheel down),
 * −1 for the previous, 0 while a trackpad's small deltas are still adding up. At most one step
 * per event, and a step spends all the travel, so a large notch is still one option.
 */
export function wheelStep(latch: WheelLatch | undefined, target: object | undefined, event: WheelInput): { latch: WheelLatch; step: -1 | 0 | 1; take: boolean } {
  const time = event.timeStamp;
  const continuing = latch !== undefined && time - latch.time <= WHEEL_IDLE_MS;
  if (target === undefined) return { latch: { page: true, target, time, travel: 0 }, step: 0, take: false };
  if (continuing && latch.page) return { latch: { ...latch, time }, step: 0, take: false };
  const carried = continuing && latch.target === target ? latch.travel : 0;
  if (event.deltaY === 0) return { latch: { page: false, target, time, travel: carried }, step: 0, take: false };
  const notches = event.deltaY / (NOTCH[event.deltaMode as 0 | 1 | 2] ?? NOTCH[0]);
  // Turning back starts the travel again rather than first unwinding what was there.
  const travel = Math.sign(carried) === Math.sign(notches) ? carried + notches : notches;
  if (Math.abs(travel) < 1) return { latch: { page: false, target, time, travel }, step: 0, take: true };
  return { latch: { page: false, target, time, travel: 0 }, step: travel > 0 ? 1 : -1, take: true };
}

/** The index a step from `from` lands on, passing over options that may not be picked; `undefined` past either end. */
export function stepIndex(eligible: readonly boolean[], from: number, step: -1 | 1): number | undefined {
  for (let i = from + step; i >= 0 && i < eligible.length; i += step) if (eligible[i] === true) return i;
  return undefined;
}

/** The select a wheel event is over, if it takes the wheel: enabled, a drop-down, shown, not inert and not opted out. */
function selectUnder(event: WheelEvent): HTMLSelectElement | undefined {
  // Ctrl with the wheel (or a trackpad pinch) is the browser's zoom.
  if (event.ctrlKey) return undefined;
  const path = event.composedPath();
  const select = path.find((node): node is HTMLSelectElement => node instanceof HTMLSelectElement);
  if (select === undefined || select.multiple || select.size > 1 || select.hasAttribute("data-no-wheel")) return undefined;
  // `:disabled` covers a disabled fieldset around the select as well as its own attribute.
  if (select.matches(":disabled") || !select.checkVisibility()) return undefined;
  if (path.some((node) => node instanceof HTMLElement && node.inert)) return undefined;
  return select;
}

const pickable = (option: HTMLOptionElement) => !option.matches(":disabled") && !option.hidden && option.closest("optgroup[hidden]") === null && !option.hasAttribute("data-no-wheel");

let installed = false;

/**
 * Installs the listener once. It is not passive, so it can keep the page still while the wheel
 * turns a select; at the ends of the list it keeps doing so, as a slider does, rather than
 * suddenly scrolling the page out from under the pointer.
 */
export function installSelectWheel(): void {
  if (installed) return;
  installed = true;
  let latch: WheelLatch | undefined;
  document.addEventListener(
    "wheel",
    (event) => {
      const select = selectUnder(event);
      const result = wheelStep(latch, select, event);
      latch = result.latch;
      if (select === undefined || !result.take) return;
      event.preventDefault();
      if (result.step === 0) return;
      const index = stepIndex([...select.options].map(pickable), select.selectedIndex, result.step);
      if (index === undefined) return;
      select.selectedIndex = index;
      // As a pick from the list: `input` crosses shadow roots, `change` does not.
      select.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
      select.dispatchEvent(new Event("change", { bubbles: true }));
    },
    { passive: false },
  );
}
