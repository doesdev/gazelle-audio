// <ga-notices>: errors and warnings from the store (a failed save, a lagging connection), shown in
// the corner until dismissed.

import { h } from "../core/dom.ts";
import { GaElement, sheet, useStore } from "./element.ts";

export class GaNotices extends GaElement {
  static override styles = [
    sheet(`
      :host { position: fixed; right: 12px; bottom: 12px; display: grid; gap: 6px; max-width: min(360px, calc(100vw - 24px)); z-index: 10; }
      .notice {
        display: flex;
        align-items: flex-start;
        gap: 8px;
        padding: 8px 10px;
        border-left: 4px solid var(--ga-notice-info);
        border-radius: 3px;
        background: var(--ga-surface-raised);
        box-shadow: 0 4px 16px rgb(0 0 0 / 0.35);
      }
      .notice[data-level="error"] { border-left-color: var(--ga-notice-error); }
      .notice[data-level="warning"] { border-left-color: var(--ga-notice-warning); }
      .text { flex: 1; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    this.watch(() => {
      this.root.replaceChildren(
        ...store.notices.value.map((notice) =>
          h(
            "div",
            { class: "notice", role: notice.level === "error" ? "alert" : "status", "data-level": notice.level },
            h("span", { class: "text" }, notice.message),
            h("button", { type: "button", "aria-label": "Dismiss", "on:click": () => store.dismiss(notice.id) }, "×"),
          ),
        ),
      );
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-notices": GaNotices;
  }
}
