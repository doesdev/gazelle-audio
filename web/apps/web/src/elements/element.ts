// The base for every <ga-*> element: an open shadow root with the shared stylesheet plus the
// element's own, effects that are disposed when the element leaves the page, and access to the
// app's store (provided once by the entry point).

import { effect } from "../core/signal.ts";
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

/**
 * Makes a text input commit like a name field: Enter or leaving the field commits the value,
 * Escape puts back the saved one. Relying on `change` alone left Enter doing nothing in some
 * browsers until focus moved.
 */
export function commitOnEnter(input: HTMLInputElement, commit: (value: string) => void, saved: () => string): void {
  let committed = saved();
  const apply = () => {
    if (input.value === committed) return;
    committed = input.value;
    commit(input.value);
  };
  input.addEventListener("focus", () => {
    committed = saved();
  });
  input.addEventListener("change", apply);
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      apply();
      input.blur();
    } else if (event.key === "Escape") {
      input.value = saved();
      committed = input.value;
      input.blur();
    }
  });
}

export function sheet(css: string): CSSStyleSheet {
  const stylesheet = new CSSStyleSheet();
  stylesheet.replaceSync(css);
  return stylesheet;
}
