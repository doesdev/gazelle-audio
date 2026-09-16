// <ga-section heading="Devices" [collapsed]>: a titled bar with a disclosure arrow that shows or
// hides its content. Controls for the section go in slot="actions" at the bar's right end. It fires
// `toggle` when the person opens or closes it.

import { h } from "../core/dom.ts";
import { GaElement, sheet } from "./element.ts";

export class GaSection extends GaElement {
  static observedAttributes = ["heading", "collapsed"];
  static override styles = [
    sheet(`
      :host { display: block; }
      .bar {
        display: flex;
        align-items: center;
        gap: 6px;
        min-height: 24px;
        padding: 0 4px 0 0;
        border-radius: 3px;
        background: var(--ga-section-header);
        color: var(--ga-section-header-text);
      }
      .toggle {
        display: flex;
        flex: 1;
        align-items: center;
        gap: 6px;
        min-height: 24px;
        padding: 0 8px;
        border: 0;
        background: transparent;
        color: inherit;
        text-align: left;
      }
      .toggle:hover:not(:disabled) { background: transparent; }
      .disclosure {
        width: 0;
        height: 0;
        border-left: 4px solid transparent;
        border-right: 4px solid transparent;
        border-top: 5px solid currentColor;
        transition: transform 120ms ease;
      }
      :host([collapsed]) .disclosure { transform: rotate(-90deg); }
      .title { font-size: 13px; }
      .body { padding: 4px 0 8px; }
      :host([collapsed]) .body { display: none; }
    `),
  ];

  #toggle: HTMLButtonElement | undefined;
  #title: HTMLSpanElement | undefined;

  get collapsed(): boolean {
    return this.hasAttribute("collapsed");
  }

  set collapsed(value: boolean) {
    this.toggleAttribute("collapsed", value);
  }

  protected override render(): void {
    this.#title = h("span", { class: "title" }, this.getAttribute("heading") ?? "");
    this.#toggle = h(
      "button",
      {
        class: "toggle",
        type: "button",
        "aria-expanded": String(!this.collapsed),
        "on:click": () => {
          this.collapsed = !this.collapsed;
          this.dispatchEvent(new Event("toggle"));
        },
      },
      h("span", { class: "disclosure", "aria-hidden": "true" }),
      this.#title,
    );
    this.root.replaceChildren(h("div", { class: "bar" }, this.#toggle, h("slot", { name: "actions" })), h("div", { class: "body", part: "body" }, h("slot")));
  }

  attributeChangedCallback(): void {
    this.#toggle?.setAttribute("aria-expanded", String(!this.collapsed));
    if (this.#title !== undefined) this.#title.textContent = this.getAttribute("heading") ?? "";
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-section": GaSection;
  }
}
