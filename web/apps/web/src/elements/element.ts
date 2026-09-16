// The base for every <ga-*> element: an open shadow root with the shared stylesheet plus the
// element's own, effects that are disposed when the element leaves the page, and access to the
// app's store (provided once by the entry point).

import { effect, type Signal } from "../core/signal.ts";
import type { Store } from "../store/store.ts";
import { shared } from "./styles.ts";

let appStore: Store | undefined;

export function provideStore(store: Store): void {
  appStore = store;
}

export function useStore(): Store {
  if (appStore === undefined) throw new Error("the store has not been provided; call provideStore() before adding elements");
  return appStore;
}

export class GaElement extends HTMLElement {
  static styles: readonly CSSStyleSheet[] = [];
  readonly root: ShadowRoot;
  #disposers: (() => void)[] = [];

  constructor() {
    super();
    this.root = this.attachShadow({ mode: "open" });
    this.root.adoptedStyleSheets = [shared, ...(this.constructor as typeof GaElement).styles];
  }

  connectedCallback(): void {
    this.render();
  }

  disconnectedCallback(): void {
    for (const dispose of this.#disposers.splice(0)) dispose();
  }

  /** Runs `fn` now and whenever what it reads changes, until the element is removed. */
  protected watch(fn: () => void | (() => void)): void {
    this.#disposers.push(effect(fn));
  }

  protected onDisconnect(fn: () => void): void {
    this.#disposers.push(fn);
  }

  protected render(): void {}
}

/** Whether the latest press was on a link: leaving a field for one is leaving the page, not committing. */
let pressedLink = false;
let tracking = false;

function trackLinkPresses(): void {
  if (tracking || typeof document === "undefined") return;
  tracking = true;
  // Capture, and the composed path, so a link in any open shadow root counts. The field's blur
  // comes after the press (on mousedown, or on click for touch), so this is known by then.
  document.addEventListener("pointerdown", (event) => (pressedLink = event.composedPath().some((n) => n instanceof HTMLAnchorElement && n.hasAttribute("href"))), { capture: true });
  document.addEventListener("keydown", () => (pressedLink = false), { capture: true });
}

/**
 * Makes a text input commit like a name field: Enter or leaving the field commits the value,
 * Escape puts back the saved one. Relying on `change` alone left Enter doing nothing in some
 * browsers until focus moved.
 *
 * With `draft` (view state, P80), a value typed and not yet committed is kept there and put back
 * when the field is built again, so a half-typed name survives leaving the page. Leaving the page
 * does not commit it: a field left for a link (the header, the device list) keeps its draft, and
 * a page going away takes its fields without committing them. Enter commits the draft, Escape
 * drops it. Returns how to show the saved value: not over a field being edited or holding a draft.
 */
export function commitOnEnter(input: HTMLInputElement, commit: (value: string) => void, saved: () => string, draft?: Signal<string | undefined>): (value: string) => void {
  trackLinkPresses();
  let committed = saved();
  const kept = draft?.peek();
  if (kept !== undefined) input.value = kept;
  const apply = () => {
    if (draft !== undefined) draft.value = undefined;
    if (input.value === committed) return;
    committed = input.value;
    commit(input.value);
  };
  input.addEventListener("focus", () => {
    committed = saved();
  });
  input.addEventListener("input", () => {
    if (draft !== undefined) draft.value = input.value === saved() ? undefined : input.value;
  });
  // `blur` rather than `change`: a draft put back is committed by leaving the field too, though it
  // was not typed since the field took focus. A field removed with its page gets no blur (checked
  // by e2e), so a page going away commits nothing.
  input.addEventListener("blur", () => {
    if (!pressedLink) apply();
  });
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      apply();
      input.blur();
    } else if (event.key === "Escape") {
      input.value = saved();
      committed = input.value;
      input.blur(); // which drops the draft, as there is nothing left to commit
    }
  });
  return (value) => {
    if ((input.getRootNode() as Document | ShadowRoot).activeElement !== input && draft?.peek() === undefined) input.value = value;
  };
}

export function sheet(css: string): CSSStyleSheet {
  const stylesheet = new CSSStyleSheet();
  stylesheet.replaceSync(css);
  return stylesheet;
}
