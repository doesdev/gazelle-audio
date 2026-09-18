// <ga-explain>: the explain mode (the user, 2026-09-18: "an optional (off by default) explain
// anything tooltip"). It sits in the header: a ? button turns the mode on and off, remembered per
// browser, and while it is on an i button beside it makes taps explain instead of act, for a screen
// with no hover.
//
// While the mode is on, hovering or focusing (from the keyboard) anything that carries a key,
// `data-explain="…"`, shows a small panel saying what it is, what changing it does and what to watch
// for. The text is the catalogue's (`explain-catalogue.ts`), a chunk of its own fetched the first time
// the mode is turned on, so the app does not carry it for everyone who never does.
//
// One set of listeners on the window finds the key with the event's composed path, so an element three
// shadow roots down is found without any element knowing about the mode. They only ever listen: they
// never cancel a click, a drag or the wheel, except while the i button is pressed, when a tap is meant
// for the panel. A press anywhere hides the panel and nothing shows again until it is released, so a
// fader or pan being dragged is never covered. The panel takes no pointer events and is placed beside
// what it explains, never over it. Escape hides it. While the mode is off, none of this is listening.

import { h } from "../core/dom.ts";
import { signal } from "../core/signal.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { adds, explainKeyIn, placePanel, resolveExplanation, type Catalogue } from "./explain-rules.ts";

/** How long the pointer rests on something before its panel shows, when none is showing yet. */
const SHOW_DELAY_MS = 350;
/** How long the panel stays after the pointer leaves, so moving between neighbours does not flicker. */
const HIDE_DELAY_MS = 150;

/** The catalogue, once fetched; the fetch is started the first time the mode is on. */
const catalogue = signal<Catalogue | undefined>(undefined);
let fetching: Promise<void> | undefined;

function loadCatalogue(): Promise<void> {
  fetching ??= import("./explain-catalogue.ts").then(
    (module) => {
      catalogue.value = module.CATALOGUE;
    },
    (error: unknown) => {
      // Tried again the next time the mode is turned on.
      fetching = undefined;
      throw error;
    },
  );
  return fetching;
}

interface Shown {
  target: Element;
  key: string;
  name: string | undefined;
  via: "hover" | "focus" | "tap";
}

export class GaExplain extends GaElement {
  static override styles = [
    sheet(`
      :host { display: inline-flex; align-items: center; gap: 2px; }
      .toggle, .pick {
        min-width: 24px;
        min-height: 24px;
        padding: 0 7px;
        border-radius: 12px;
        font-weight: 700;
        line-height: 1;
        color: var(--ga-text-secondary);
      }
      .toggle[aria-pressed="true"], .pick[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); border-color: var(--ga-accent); }
      .pick { font-style: italic; font-family: Georgia, serif; }
      .panel {
        position: fixed;
        inset: auto;
        box-sizing: border-box;
        width: max-content;
        max-width: min(320px, calc(100vw - 16px));
        margin: 0;
        padding: 8px 10px;
        border: 1px solid var(--ga-border-strong);
        border-radius: 4px;
        background: var(--ga-surface-raised);
        color: var(--ga-text-primary);
        box-shadow: 0 6px 18px rgb(0 0 0 / 0.4);
        font-size: 12px;
        line-height: 1.4;
        /* Never in the way: a click, drag or wheel over it goes to whatever is underneath. */
        pointer-events: none;
        user-select: none;
      }
      .panel p { margin: 0; }
      .panel p + p { margin-top: 4px; }
      .title { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 13px; font-weight: 600; }
      .effect { color: var(--ga-text-secondary); }
      .watch { padding-left: 6px; border-left: 3px solid var(--ga-notice-warning); }
      .watch strong { color: var(--ga-notice-warning); font-weight: 600; }
      .now { padding-top: 4px; border-top: 1px solid var(--ga-border-subtle); color: var(--ga-text-secondary); font-style: italic; }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const toggle = h(
      "button",
      {
        type: "button",
        class: "toggle",
        "aria-label": "Explain mode",
        title: "Explain mode: point at or focus anything to see what it does",
        "data-explain": "header.explain",
        "on:click": () => store.setExplainMode(!store.explainMode.peek()),
      },
      "?",
    );
    const picking = signal(false);
    const pick = h(
      "button",
      {
        type: "button",
        class: "pick",
        "aria-label": "Explain by tapping",
        title: "While this is on, a tap or click explains a control instead of using it",
        "data-explain": "header.explain-tap",
        hidden: true,
        "on:click": () => (picking.value = !picking.peek()),
      },
      "i",
    );
    const title = h("p", { class: "title" });
    const what = h("p", { class: "what" });
    const effect = h("p", { class: "effect" });
    const watch = h("p", { class: "watch" });
    const now = h("p", { class: "now" });
    const panel = h("div", { class: "panel", popover: "manual", role: "tooltip", "aria-live": "polite", "data-testid": "explain-panel" }, title, what, effect, watch, now);
    this.root.replaceChildren(toggle, pick, panel);

    let shown: Shown | undefined;
    let wanted: Shown | undefined;
    let dismissed: Element | undefined;
    /** What the pointer rests on, waiting out the delay before its panel shows. */
    let pending: Element | undefined;
    let pressed = false;
    let showTimer: ReturnType<typeof setTimeout> | undefined;
    let hideTimer: ReturnType<typeof setTimeout> | undefined;
    const cancelTimers = () => {
      clearTimeout(showTimer);
      clearTimeout(hideTimer);
      showTimer = hideTimer = undefined;
      pending = undefined;
    };

    // An element's own tooltip (its title) says what it is doing now: "Sums Mix 1 to mono, so HP1
    // goes mono too", why a meter reads nothing. The panel shows it as its last line, and takes it
    // off the element while the panel is up, so the browser's own tooltip does not land on top.
    let borrowed: { element: Element; title: string } | undefined;
    const giveBack = () => {
      if (borrowed !== undefined && !borrowed.element.hasAttribute("title")) borrowed.element.setAttribute("title", borrowed.title);
      borrowed = undefined;
    };

    const hide = () => {
      cancelTimers();
      giveBack();
      shown = wanted = undefined;
      if (panel.matches(":popover-open")) panel.hidePopover();
    };

    const show = (next: Shown) => {
      cancelTimers();
      wanted = next;
      const entries = catalogue.peek();
      // Not here yet: shown when it arrives, if still wanted.
      if (entries === undefined) return;
      const entry = entries[next.key];
      if (entry === undefined || !next.target.isConnected) {
        hide();
        return;
      }
      const text = resolveExplanation(entry, next.name);
      title.textContent = text.title;
      what.textContent = text.what;
      effect.textContent = text.effect ?? "";
      effect.hidden = text.effect === undefined;
      watch.replaceChildren(h("strong", {}, "Watch for: "), text.watch ?? "");
      watch.hidden = text.watch === undefined;
      giveBack();
      const own = next.target.getAttribute("title") ?? "";
      now.textContent = own;
      now.hidden = !adds(own, text);
      if (own !== "") {
        borrowed = { element: next.target, title: own };
        next.target.removeAttribute("title");
      }
      shown = next;
      panel.style.width = "";
      if (!panel.matches(":popover-open")) panel.showPopover();
      place();
    };

    const place = () => {
      if (shown === undefined) return;
      const rect = shown.target.getBoundingClientRect();
      const viewport = { width: document.documentElement.clientWidth, height: document.documentElement.clientHeight };
      let at = placePanel(rect, { width: panel.offsetWidth, height: panel.offsetHeight }, viewport);
      if (at.width < panel.offsetWidth) {
        // Narrowed to the window, it wraps to more lines: place it again at its new height.
        panel.style.width = `${at.width}px`;
        at = placePanel(rect, { width: at.width, height: panel.offsetHeight }, viewport);
      }
      panel.style.left = `${at.left}px`;
      panel.style.top = `${at.top}px`;
      panel.dataset["side"] = at.side;
    };

    this.watch(() => {
      const entries = catalogue.value;
      if (entries !== undefined && wanted !== undefined && shown === undefined) show(wanted);
    });

    /** What an event is about: the nearest keyed element on its path, if any. */
    const about = (event: Event, via: Shown["via"]): Shown | undefined => {
      const path = event.composedPath();
      const found = explainKeyIn(path);
      return found === undefined ? undefined : { target: path[found.index] as Element, key: found.key, name: found.name, via };
    };
    /** The same for an element found some other way: its path out through its shadow roots' hosts. */
    const aboutNode = (node: Element): Shown | undefined => {
      const path: Node[] = [];
      for (let at: Node | null = node; at !== null; at = at instanceof ShadowRoot ? at.host : at.parentNode) path.push(at);
      const found = explainKeyIn(path);
      return found === undefined ? undefined : { target: path[found.index] as Element, key: found.key, name: found.name, via: "focus" };
    };
    const ours = (event: Event) => event.composedPath().some((node) => node === toggle || node === pick);

    // The pointer is followed with pointermove, not pointerover: moving between two elements of one
    // shadow root, pointerover (like focusin) has its target and related target retargeted to the
    // same host, so by the DOM's own rule it never reaches the document. pointermove has no related
    // target and always arrives.
    const onMove =(event: PointerEvent) => {
      if (event.pointerType === "touch" || pressed || picking.peek()) return;
      const next = about(event, "hover");
      if (next === undefined) {
        pending = undefined;
        clearTimeout(showTimer);
        showTimer = undefined;
        if (shown?.via === "hover" && hideTimer === undefined) hideTimer = setTimeout(hide, HIDE_DELAY_MS);
        return;
      }
      if (next.target === shown?.target) {
        clearTimeout(hideTimer);
        hideTimer = undefined;
        return;
      }
      if (next.target === pending || next.target === dismissed) return;
      dismissed = undefined;
      pending = next.target;
      clearTimeout(showTimer);
      // Moving from one explained thing to the next shows the next at once.
      showTimer = setTimeout(() => {
        pending = undefined;
        show(next);
      }, shown === undefined ? SHOW_DELAY_MS : 0);
    };
    const onLeave = (event: PointerEvent) => {
      if (event.relatedTarget === null && shown?.via === "hover") hide();
    };

    // Focus is followed after each key, for the same reason as the pointer: focusin between two
    // elements of one shadow root never reaches the document. Only focus the keyboard moved counts
    // (`:focus-visible`); a click's is the pointer's, which hovering already explains.
    let focused: Element | undefined;
    const onKeyUp = () => {
      if (pressed || picking.peek()) return;
      const now = deepActiveElement();
      if (now === focused) return;
      focused = now;
      const next = now !== undefined && now.matches(":focus-visible") ? aboutNode(now) : undefined;
      if (next === undefined) {
        if (shown?.via === "focus") hide();
        return;
      }
      dismissed = undefined;
      show(next);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (picking.peek()) {
        picking.value = false;
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (shown === undefined) return;
      // The first Escape is the panel's: it goes, and the control keeps focus and its own Escape for next time.
      dismissed = shown.target;
      hide();
      event.preventDefault();
      event.stopPropagation();
    };

    // While the i button is pressed, a tap is for the panel: it and the mouse events that follow it
    // never reach the control. Anything else is only watched.
    let swallowing = false;
    const swallow = (event: Event) => {
      if (!swallowing) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.type === "click") swallowing = false;
    };
    const onDown = (event: PointerEvent) => {
      if (picking.peek() && !ours(event)) {
        event.preventDefault();
        event.stopPropagation();
        swallowing = true;
        const next = about(event, "tap");
        if (next === undefined) hide();
        else show(next);
        return;
      }
      swallowing = false;
      pressed = true;
      hide();
    };
    const onUp = (event: PointerEvent) => {
      if (swallowing) {
        event.preventDefault();
        event.stopPropagation();
      }
      pressed = false;
    };
    const onScroll = () => {
      if (shown !== undefined) hide();
    };

    const listeners: [EventTarget, string, EventListener, AddEventListenerOptions][] = [
      [window, "pointermove", onMove as EventListener, { capture: true, passive: true }],
      [window, "pointerout", onLeave as EventListener, { capture: true, passive: true }],
      [window, "keyup", onKeyUp, { capture: true, passive: true }],
      [window, "keydown", onKey as EventListener, { capture: true }],
      [window, "pointerdown", onDown as EventListener, { capture: true }],
      [window, "pointerup", onUp as EventListener, { capture: true }],
      [window, "pointercancel", onUp as EventListener, { capture: true }],
      [window, "mousedown", swallow, { capture: true }],
      [window, "mouseup", swallow, { capture: true }],
      [window, "click", swallow, { capture: true }],
      [window, "dblclick", swallow, { capture: true }],
      [window, "scroll", onScroll, { capture: true, passive: true }],
      [window, "resize", onScroll, { passive: true }],
    ];

    this.watch(() => {
      const on = store.explainMode.value;
      toggle.setAttribute("aria-pressed", String(on));
      pick.hidden = !on;
      if (!on) {
        picking.value = false;
        hide();
        return;
      }
      void loadCatalogue().catch(() => store.reportError("The explanations could not be loaded. Turn the explain mode off and on to try again."));
      for (const [target, type, listener, options] of listeners) target.addEventListener(type, listener, options);
      return () => {
        for (const [target, type, listener, options] of listeners) target.removeEventListener(type, listener, options);
        hide();
        pressed = swallowing = false;
      };
    });
    this.watch(() => {
      const on = picking.value;
      pick.setAttribute("aria-pressed", String(on));
      if (!on) hide();
    });
    this.onDisconnect(hide);
  }
}

/** The focused element, inside whichever shadow roots hold it. */
function deepActiveElement(): Element | undefined {
  let active = document.activeElement;
  while (active?.shadowRoot?.activeElement) active = active.shadowRoot.activeElement;
  return active ?? undefined;
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-explain": GaExplain;
  }
}
