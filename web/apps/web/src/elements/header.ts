// <ga-header>: brand, page tabs, and the hardware-safety badges that must always be
// visible: which backend is driving devices, dry-run, and the connection state, with the running
// version beside them while the explain mode is on, and, when there is one, what the updater
// wants: a version ready to restart into, one on its way, or one that failed. The
// preferences sit at the end, what a double-click does to a level and the theme, then slot="menu",
// where the app puts its sidebar button for phones.
//
// Narrower than a laptop, the bar takes two lines: the brand and badges above, the page tabs and
// theme picker below, where the tabs scroll sideways within their line when they do not fit.

import { h } from "../core/dom.ts";
import { bindConfirm } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { href, PAGES, route } from "./router.ts";

const STATUS_TEXT = { open: "Connected", reconnecting: "Reconnecting…", closed: "Disconnected" } as const;

/** Each page tab's key for the explain mode. */
const PAGE_KEYS: Record<string, string> = {
  devices: "header.page.devices",
  workspace: "header.page.workspace",
  inputs: "header.page.inputs",
  outputs: "header.page.outputs",
  mixer: "header.page.mixer",
  routing: "header.page.routing",
  effects: "header.page.effects",
};

export class GaHeader extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; background: var(--ga-surface-panel); border-bottom: 1px solid var(--ga-surface-background); }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 0 12px; min-height: 40px; padding: 0 12px; }
      .brand { font-size: 20px; letter-spacing: 0.06em; }
      nav { display: flex; gap: 2px; }
      nav a { flex: none; white-space: nowrap; }
      nav a {
        padding: 4px 10px;
        border-radius: 3px;
        color: var(--ga-text-secondary);
        font-family: "Josefin Sans Variable", system-ui, sans-serif;
        font-size: 14px;
        font-weight: 600;
      }
      nav a:hover { background: var(--ga-control-hover); color: var(--ga-text-primary); }
      nav a[aria-current="page"] { background: var(--ga-control-active); color: var(--ga-text-primary); }
      .spacer { flex: 1; }
      .badge {
        padding: 2px 7px;
        border-radius: 3px;
        font-size: 10px;
        font-weight: 700;
        letter-spacing: 0.08em;
        text-transform: uppercase;
      }
      /* The running version, left of the backend badge and only while the explain mode is on (the
         user, 2026-09-19): the dimmest text colour there is, no border and no background, so it is
         there to read and nothing more. It keeps its place in the line whether it is shown or not,
         so turning the explain mode on does not slide the badges along; the flexible gap to its
         left takes up the difference. */
      .version { visibility: hidden; color: var(--ga-text-muted); font-size: 10px; font-variant-numeric: tabular-nums; letter-spacing: 0.04em; }
      .version[data-shown] { visibility: visible; }
      /* What the updater wants, left of the version. Nothing at all while there is nothing to
         say, which is nearly always, so it takes no room rather than reserving it the way the
         version does: this line is the one that is always full, and holding a gap open for
         something seen a few times a year would push the badges about for nothing. When it does
         appear it is because something happened, which is the moment to be noticed. */
      .update { color: var(--ga-text-secondary); font-size: 10px; letter-spacing: 0.04em; white-space: nowrap; }
      button.update {
        padding: 2px 8px;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 3px;
        background: var(--ga-surface-inset);
        color: var(--ga-text-secondary);
        font-family: inherit;
        cursor: pointer;
      }
      button.update:hover { background: var(--ga-control-hover); color: var(--ga-text-primary); }
      /* Armed, and outlined as every other confirm in the app is. */
      button.update[data-armed] { border-color: var(--ga-notice-warning); color: var(--ga-notice-warning); }
      button.update[data-kind="failed"] { color: var(--ga-notice-warning); border-color: var(--ga-notice-warning); }
      .backend { background: var(--ga-surface-inset); color: var(--ga-text-secondary); border: 1px solid var(--ga-border-subtle); }
      .backend[data-backend="usb"] { color: var(--ga-notice-warning); border-color: var(--ga-notice-warning); }
      .dry-run { background: var(--ga-state-dry-run); color: var(--ga-text-inverse); }
      .status { display: flex; align-items: center; gap: 6px; color: var(--ga-text-secondary); font-size: 11px; }
      .status::before { content: ""; width: 8px; height: 8px; border-radius: 50%; background: var(--ga-connection-closed); }
      .status[data-state="open"]::before { background: var(--ga-connection-open); }
      .status[data-state="reconnecting"]::before { background: var(--ga-connection-reconnecting); }
      @media (max-width: 959px) {
        .bar { gap: 2px 8px; padding: 4px 8px; }
        /* A zero-height line break, ordered between the two lines. */
        .bar::after { content: ""; order: 1; flex: 0 0 100%; }
        nav { order: 2; flex: 1 1 0; min-width: 0; overflow-x: auto; scrollbar-width: none; }
        .theme, .reset { order: 3; max-width: 120px; }
      }
      @media (max-width: 480px) {
        .brand { font-size: 17px; }
        nav a { padding: 4px 8px; }
        .theme { max-width: 96px; }
        /* A phone has no double-click. */
        .reset { display: none; }
        /* Too little room on this line for a version nobody is looking for; the tray menu and the
           server's own /api/v1/health still say it. */
        .version { display: none; }
        /* The update prompt stays: unlike the version it is a thing to do, not a thing to read,
           and a phone is as good a place to press it from as a desk. */
        /* The dot's colour carries the state; the words stay for screen readers and the tooltip. */
        .status-text { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
      }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const links = PAGES.map((page) => h("a", { href: href({ page: page.page }), "data-page": page.page, "data-explain": PAGE_KEYS[page.page] }, page.label));
    // Which version is running, for when someone is asked. Out of the way until the explain mode is
    // on, and nothing at all until the server has said hello.
    const version = h("span", { class: "version", "aria-label": "Server version", "data-testid": "version", "data-explain": "header.version" });
    // What the updater wants, when it wants something. A button when there is something to press,
    // a readout while a download runs; both hidden the rest of the time.
    const updateButton = h("button", { type: "button", class: "update", "data-testid": "update-action", "data-explain": "header.update", hidden: true });
    const updateNote = h("span", { class: "update", role: "status", "data-testid": "update-note", "data-explain": "header.update-note", hidden: true });
    const backend = h("span", { class: "badge backend", "data-testid": "backend", "data-explain": "header.backend" });
    const dryRun = h("span", { class: "badge dry-run", "data-testid": "dry-run", "data-explain": "header.dry-run", title: "Commands report the bytes they would send; nothing is written to a device." }, "Dry run");
    const statusText = h("span", { class: "status-text" });
    const status = h("span", { class: "status", role: "status", "data-testid": "connection", "data-explain": "header.connection" }, statusText);
    const picker = h("select", { class: "theme", "aria-label": "Theme", "data-explain": "header.theme", "on:change": (event) => store.selectTheme((event.target as HTMLSelectElement).value) });
    // What a double-click on a level does (the user, 2026-09-18): a safe -20 dB unless unity is chosen.
    // Not by the wheel: it is a safety setting, and the header is where the pointer passes.
    const reset = h(
      "select",
      {
        class: "reset",
        "aria-label": "Double-click on a level",
        title: "What a double-click on a fader, volume, send or return does. Ctrl/Cmd+click always sets unity.",
        "data-testid": "double-click-level",
        "data-no-wheel": true,
        "data-explain": "header.double-click",
        "on:change": () => store.setDoubleClickUnity(reset.value === "unity"),
      },
      h("option", { value: "safe" }, "Double-click: safe level"),
      h("option", { value: "unity" }, "Double-click: unity"),
    );

    this.root.replaceChildren(h("div", { class: "bar" }, h("span", { class: "brand title" }, "Gazelle"), h("nav", { "aria-label": "Pages" }, links), h("span", { class: "spacer" }), updateNote, updateButton, version, backend, dryRun, status, reset, picker, h("slot", { name: "menu" })));

    this.watch(() => {
      // A surface is opened from the Workspace page, so that tab stays marked while one is shown.
      const current = route.value.page === "surface" ? "workspace" : route.value.page;
      for (const link of links) link.toggleAttribute("aria-current", link.dataset["page"] === current);
      for (const link of links) if (link.dataset["page"] === current) link.setAttribute("aria-current", "page");
    });
    this.watch(() => {
      const info = store.server.value;
      backend.textContent = info.backend || "unknown";
      backend.dataset["backend"] = info.backend;
      dryRun.hidden = !info.dry_run;
    });
    // A restart drops every device connection for a few seconds, which is the kind of thing the
    // rest of the app makes you press twice, so it is armed the same way. Getting a download or
    // trying a failed check again costs nothing and goes on one press.
    const act = () => {
      const prompt = store.updatePrompt.peek();
      if (prompt?.act === "restart") void store.restartForUpdate();
      else if (prompt?.act === "download") void store.downloadUpdate();
      else if (prompt?.act === "check") void store.checkForUpdate();
    };
    const disarm = bindConfirm(updateButton, () => store.updatePrompt.peek()?.label ?? "", act, () => store.updatePrompt.peek()?.confirm === true);
    this.watch(() => {
      const prompt = store.updatePrompt.value;
      // A half-pressed confirm belonged to the prompt that has just been replaced.
      disarm();
      updateNote.hidden = prompt === undefined || prompt.act !== undefined;
      updateButton.hidden = prompt === undefined || prompt.act === undefined;
      if (prompt === undefined) return;
      const target = prompt.act === undefined ? updateNote : updateButton;
      target.textContent = prompt.label;
      target.title = prompt.title;
      target.dataset["kind"] = prompt.kind;
    });
    this.watch(() => {
      const running = store.server.value.version;
      version.textContent = running;
      version.title = `Gazelle ${running}`;
      version.toggleAttribute("data-shown", running !== "" && store.explainMode.value);
    });
    this.watch(() => {
      const state = store.status.value;
      status.dataset["state"] = state;
      statusText.textContent = STATUS_TEXT[state];
      status.title = STATUS_TEXT[state];
    });
    this.watch(() => {
      reset.value = store.doubleClickUnity.value ? "unity" : "safe";
    });
    this.watch(() => {
      const { themes } = store.themeCatalog.value;
      const current = store.theme.value.id;
      picker.replaceChildren(...themes.map((theme) => h("option", { value: theme.id, selected: theme.id === current }, theme.name)));
      picker.value = current;
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-header": GaHeader;
  }
}
