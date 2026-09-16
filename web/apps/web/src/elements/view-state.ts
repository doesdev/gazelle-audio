// Keeping a page's view state across rebuilds (decision P71). The router builds a new page element
// on every change of address, and a page element stops everything it follows when it goes, so
// nothing runs for a page that is not shown. What a person would expect to find again (scroll,
// open sections, a selection in progress) is kept in the store's `view` state instead, and these
// bind an element to it: each puts the element back as it was, then records what the person does.

import type { Signal } from "../core/signal.ts";
import type { GaSection } from "./section.ts";

/** How long a scroll offset is retried while the content that makes it reachable arrives. */
export const RESTORE_MS = 1000;

/**
 * Keeps `state` at the element's scroll offset along `axis`, and scrolls it back there now. When
 * the content is not yet long enough, the offset is retried each frame until it is reached, the
 * person scrolls, or `RESTORE_MS` passes. Returns a function that stops both.
 */
export function keepScroll(element: HTMLElement, state: Signal<number>, axis: "top" | "left" = "top"): () => void {
  const offset = axis === "top" ? "scrollTop" : "scrollLeft";
  const wanted = state.peek();
  const started = performance.now();
  let frame = 0;
  const stopRestoring = () => {
    cancelAnimationFrame(frame);
  };
  const restore = () => {
    element[offset] = wanted;
    if (Math.abs(element[offset] - wanted) <= 1 || performance.now() - started > RESTORE_MS) stopRestoring();
    else frame = requestAnimationFrame(restore);
  };
  const record = () => {
    state.value = element[offset];
  };
  const inputs = ["wheel", "pointerdown", "keydown", "touchstart"] as const;
  element.addEventListener("scroll", record, { passive: true });
  for (const type of inputs) element.addEventListener(type, stopRestoring, { passive: true });
  restore();
  return () => {
    stopRestoring();
    element.removeEventListener("scroll", record);
    for (const type of inputs) element.removeEventListener(type, stopRestoring);
  };
}

/** Opens or closes a `<details>` as `state` says, and keeps `state` as the person toggles it. */
export function keepOpen(details: HTMLDetailsElement, state: Signal<boolean>): void {
  details.open = state.peek();
  details.addEventListener("toggle", () => {
    state.value = details.open;
  });
}

/** Collapses a `<ga-section>` as `state` says, and keeps `state` as the person toggles it. */
export function keepCollapsed(section: GaSection, state: Signal<boolean>): void {
  section.collapsed = state.peek();
  section.addEventListener("toggle", () => {
    state.value = section.collapsed;
  });
}
