// <ga-mixer-dock>: the compact mixer under every page (the user's choice, 2026-09-16), so levels
// can be ridden from Inputs or Routing. It shows the device in view (the page's device if it has a
// known mixer, else the one last selected) in its selected mix, the same per-device mix the Mixer
// page's Mix buttons set, which its own small Mix menu sets too: a slim strip per channel set up in
// that mix, in the Mixer page's order, then the mix's master. Strips scroll sideways; the master
// stays at the right. A mix with no channel set up is one short line pointing to the Mixer page,
// without its master: there is nothing in it to ride, and a fader needs the full row's height.
//
// Its Show menu can put a cross-device surface there instead: the surface's
// strips at dock width, each with its device's badge (`ga-surface-strip compact`), so the Quadro's cue
// faders stay in reach on the Studio+'s Inputs page. The choice is kept per browser, and falls back
// to the device in view once the surface is deleted.
//
// On the Mixer page it is hidden and builds nothing, since the page shows the same strips in full;
// a surface in the dock is hidden, likewise, on that surface's own page.
// While hidden or collapsed it follows nothing: the report watch, the mix reads and the strips are
// released, as they are when another device comes into view. Whether it is collapsed is kept per
// browser; until someone chooses, it starts collapsed at phone width, where open it would take a
// quarter of the screen.
//
// Sources dragged from the Routing page drop here (`source-drag.ts`): each becomes a channel in the
// mix shown, fed by that source, made as the Mixer page's "+" and its Input and Main mix menus make
// one. While such a drag is over it the dock is outlined and says what a drop would do, or why it
// would not: it adds only to the device the sources belong to, so a dock showing a surface or
// another device refuses the drop and says so. A drop that would put an input into the mix twice
// waits behind Confirm, as the Input menu's does. Held over a folded dock, the drag opens it.

import { h } from "../core/dom.ts";
import { effect, untracked } from "../core/signal.ts";
import { meterDeflection } from "../store/mixer.ts";
import { displayName } from "../store/store.ts";
import { meterGradient } from "../themes/theme.ts";
import { channelStrip } from "./channel.ts";
import { CONFIRM_MS } from "./controls.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { isReady, loadElement } from "./lazy.ts";
import { href, route } from "./router.ts";
import type { GaSection } from "./section.ts";
import { activeSourceDrag, addDroppedSources, carriesSources, decodeSourceDrag, doublingsOf, dropHint, layoutForDrop, SOURCE_MIME, type SourceDrag } from "./source-drag.ts";

/** How long a drag of sources rests on a folded dock before it opens. */
const OPEN_ON_HOVER_MS = 600;
/** How long a drop that would double an input waits for Confirm: a few seconds more than a menu's, since the reason must be read first. */
const DROP_CONFIRM_MS = 2 * CONFIRM_MS;

export class GaMixerDock extends GaElement {
  static override styles = [
    sheet(`
      :host { position: relative; display: block; padding: 6px 8px; border-top: 1px solid var(--ga-surface-background); background: var(--ga-surface-panel); }
      :host([hidden]) { display: none; }
      /* A drag of routing sources over the dock: outlined, with what a drop would do or why not. */
      :host([data-drop]) { outline: 2px dashed var(--ga-accent); outline-offset: -3px; }
      :host([data-drop="refused"]) { outline-color: var(--ga-state-mute); }
      .drop-hint { position: absolute; top: 6px; left: 50%; z-index: 2; max-width: calc(100% - 32px); padding: 2px 10px; border-radius: 3px; background: var(--ga-accent); color: var(--ga-accent-text); font-size: 11px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; transform: translateX(-50%); pointer-events: none; }
      :host([data-drop="refused"]) .drop-hint { background: var(--ga-state-mute); color: var(--ga-text-inverse); }
      .drop-hint[hidden], .drop-confirm[hidden] { display: none; }
      /* A drop that would double an input: the reason and Confirm, outlined as an armed 48V is. */
      .drop-confirm { display: flex; align-items: center; gap: 6px; margin-bottom: 4px; padding: 2px 6px; border-radius: 3px; outline: 2px dashed var(--ga-state-mute); outline-offset: -2px; font-size: 11px; }
      .drop-confirm .reason { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .drop-confirm button { min-height: 20px; padding: 0 8px; font-size: 10px; font-weight: 700; }
      .actions { display: flex; align-items: center; gap: 6px; min-width: 0; }
      .device { font-size: 11px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .actions select { height: 18px; min-height: 18px; padding: 0 4px; font-size: 10px; color: var(--ga-text-secondary); background: transparent; border: 1px solid var(--ga-border-subtle); border-radius: 2px; }
      .actions select:hover { color: var(--ga-text-primary); }
      /* A fixed height that leaves the fader about 130 px of travel. */
      .strips {
        display: flex;
        gap: 2px;
        height: 200px;
        padding: 4px 0 4px 4px;
        overflow-x: auto;
        overflow-y: hidden;
        border-radius: 3px;
        background: var(--ga-surface-inset);
      }
      .strips ga-strip { flex: 0 0 44px; }
      .strips ga-surface-strip { min-height: 0; }
      .master { position: sticky; right: 0; display: flex; margin-left: auto; padding: 0 4px 0 6px; background: var(--ga-surface-inset); box-shadow: -8px 0 8px -4px rgb(0 0 0 / 0.5); }
      .master ga-strip { flex: 0 0 48px; }
      /* Nothing to show is one line, not a row of strip height. */
      .strips.empty { height: auto; padding: 0; }
      .empty .placeholder { flex: 1; padding: 4px 8px; font-size: 11px; }
      .empty a { color: var(--ga-accent); }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const device = h("span", { class: "device muted" });
    const mixSelect = h("select", { "aria-label": "Dock mix", "data-testid": "dock-mix-select", "data-explain": "dock.mix" });
    const sourceSelect = h("select", { "aria-label": "Dock shows", "data-testid": "dock-source-select", "data-explain": "dock.source", "on:change": () => store.setMixerDockSurface(sourceSelect.value === "" ? undefined : sourceSelect.value) });
    const actions = h("div", { class: "actions", slot: "actions" }, sourceSelect, device, mixSelect);
    const strips = h("div", { class: "strips", "data-testid": "dock-strips" });
    const dropReason = h("span", { class: "reason", "data-testid": "dock-drop-reason" });
    const dropConfirm = h("button", { type: "button", "data-testid": "dock-drop-confirm", "data-explain": "dock.drop-confirm" }, "Confirm");
    const dropCancel = h("button", { type: "button", "data-testid": "dock-drop-cancel", "data-explain": "dock.drop-cancel" }, "Cancel");
    const dropBar = h("div", { class: "drop-confirm", role: "alert", hidden: true }, dropReason, dropConfirm, dropCancel);
    const hint = h("div", { class: "drop-hint", "data-testid": "dock-drop-hint", "aria-live": "polite", hidden: true });
    const section = h("ga-section", { heading: "Mixer", explain: "dock.section" }, actions, dropBar, strips) as GaSection;
    section.collapsed = store.mixerDockCollapsed.peek();
    section.addEventListener("toggle", () => store.setMixerDockCollapsed(section.collapsed));
    this.root.replaceChildren(section, hint);

    // What the shown device's dock holds on to: its mix reads, meter report and strips. Released
    // when another device is shown, the dock is collapsed or the Mixer page opens.
    let held: (() => void)[] = [];
    const release = () => {
      for (const dispose of held.splice(0)) dispose();
      strips.replaceChildren();
    };
    /** Only a message in the row: it takes one line. */
    const showMessage = (...message: (Node | string)[]) => {
      strips.classList.add("empty");
      strips.replaceChildren(h("p", { class: "placeholder" }, ...message));
    };
    this.onDisconnect(release);
    const follow = (deviceId: string): (() => void)[] => {
      const channels = store.channels(deviceId);
      const own: (() => void)[] = [];
      // Levels come from the device, read once as the Mixer page reads them.
      own.push(
        effect(() => {
          if (store.mixesToRead(deviceId)) untracked(() => void store.readMixes(deviceId));
        }),
      );
      // Strips meter their inputs from the status report, followed while the dock shows the mix,
      // with the interface's own mixer channel meters filling in the inputs it does not meter by type.
      own.push(effect(() => store.mixer(deviceId, channels.meteredMix.value).activate()));
      own.push(effect(() => store.pointMeterBank(deviceId, channels.meteredMix.value)));
      own.push(
        effect(() => {
          const entry = store.devices.value.find((d) => d.id === deviceId);
          device.textContent = entry === undefined ? "" : displayName(entry, store.workspace.value);
          mixSelect.replaceChildren(...Array.from({ length: channels.mixCount }, (_, mix) => h("option", { value: String(mix) }, channels.mixName(mix))));
          mixSelect.value = String(channels.meteredMix.value);
        }),
      );
      const choose = () => {
        channels.meteredMix.value = Number(mixSelect.value);
      };
      mixSelect.addEventListener("change", choose);
      own.push(() => mixSelect.removeEventListener("change", choose));

      // Strips read their attributes once, so the row is rebuilt when what they show changes.
      let rendered = "";
      own.push(
        effect(() => {
          const mix = channels.meteredMix.value;
          const shownStrips = channels.inMix(mix).map((ch) => channelStrip(deviceId, mix, ch.slot, channels.strip(ch, mix), true));
          const mixName = channels.mixName(mix);
          const key = JSON.stringify([mixName, shownStrips]);
          if (key === rendered) return;
          rendered = key;
          // Untracked: a strip renders as it is appended, and what it reads must not rebuild the row.
          untracked(() => {
            if (shownStrips.length === 0) {
              // Short enough for one line on a phone.
              showMessage(`No channels in ${mixName}. `, h("a", { href: href({ page: "mixer", id: deviceId }), "data-explain": "dock.open-mixer" }, "Open the Mixer page"));
              return;
            }
            const master = h("div", { class: "master" }, h("ga-strip", { "device-id": deviceId, mixer: String(mix), strip: "master", label: mixName, compact: "" }));
            strips.classList.remove("empty");
            strips.replaceChildren(...shownStrips.map((attributes) => h("ga-strip", attributes)), master);
          });
        }),
      );
      return own;
    };

    // A surface: its strips at dock width, rebuilt when its strips or their order change. The
    // surface strip is not loaded with the app (`lazy.ts`): it belongs to the surface page, which
    // most sessions never open. So the dock asks for it the first time it shows one, and says so
    // in the row meanwhile. It is only ever fetched once, whichever asks first.
    const followSurface = (surfaceId: string): (() => void)[] => {
      let rendered = "";
      let alive = true;
      const paint = (name: string, ids: string[]): void => {
        if (!alive) return;
        if (ids.length === 0) {
          strips.replaceChildren(h("p", { class: "placeholder empty" }, `${name} has no strips yet. `, h("a", { href: href({ page: "surface", id: surfaceId }), "data-explain": "dock.open-surface" }, "Add some on the surface.")));
          return;
        }
        if (!isReady("ga-surface-strip")) {
          strips.replaceChildren(h("p", { class: "placeholder empty", "data-testid": "dock-loading" }, `Loading ${name}…`));
          void loadElement("ga-surface-strip").then(
            () => paint(name, ids),
            () => {
              if (alive) strips.replaceChildren(h("p", { class: "placeholder empty", role: "alert" }, `${name} could not be loaded. Reload the page to try again.`));
            },
          );
          return;
        }
        strips.replaceChildren(...ids.map((id) => h("ga-surface-strip", { "surface-id": surfaceId, "strip-id": id, compact: "" })));
      };
      return [
        effect(() => {
          const surface = store.surfaces.surface(surfaceId);
          device.textContent = surface?.name ?? "";
          const ids = (surface?.strips ?? []).map((s) => s.id);
          const key = JSON.stringify(ids);
          if (key === rendered) return;
          rendered = key;
          untracked(() => paint(surface?.name ?? "This surface", ids));
        }),
        () => (alive = false),
      ];
    };

    this.watch(() => {
      const chosen = store.mixerDockSurface.value;
      sourceSelect.replaceChildren(h("option", { value: "" }, "This device"), ...store.surfaces.list.value.map((surface) => h("option", { value: surface.id }, surface.name)));
      sourceSelect.value = chosen ?? "";
      sourceSelect.disabled = !store.connected.value;
    });

    // What a drop of routing sources lands in: the device the dock shows, or a surface, or nothing.
    let showing: { surface: string } | { deviceId: string } | undefined;

    let shown: string | undefined;
    this.watch(() => {
      const current = route.value;
      const surface = store.mixerDockSurface.value;
      // Hidden where the page already shows the same strips in full.
      const repeated = surface === undefined ? current.page === "mixer" : current.page === "surface" && current.id === surface;
      const collapsed = store.mixerDockCollapsed.value;
      const known = store.devices.value.filter((d) => d.family !== null);
      const id = known.find((d) => d.id === current.id)?.id ?? store.deviceInView(true);
      this.hidden = repeated;
      // Collapsed, the menus are not filled in, so its bar shows no empty menu.
      actions.hidden = collapsed || (surface === undefined && id === undefined && store.surfaces.list.value.length === 0);
      mixSelect.hidden = surface !== undefined || id === undefined;
      device.hidden = surface === undefined && id === undefined;
      showing = repeated ? undefined : surface !== undefined ? { surface } : id !== undefined ? { deviceId: id } : undefined;
      const key = repeated || collapsed ? "" : surface !== undefined ? `surface:${surface}` : `device:${id ?? ""}`;
      if (key === shown) return;
      shown = key;
      untracked(() => {
        release();
        section.collapsed = collapsed;
        if (key === "") return;
        if (surface !== undefined) {
          held = followSurface(surface);
          return;
        }
        if (id === undefined) {
          showMessage("No device with a known mixer is connected.");
          return;
        }
        held = follow(id);
      });
    });

    this.#dropTarget(store, () => showing, section, hint, { bar: dropBar, reason: dropReason, confirm: dropConfirm, cancel: dropCancel });

    this.watch(() => {
      // Meter gradient stops are dBFS; place them on the scale the meters use, as the Mixer page does.
      this.style.setProperty("--mixer-meter-gradient", meterGradient(store.theme.value.meter.gradient, undefined, "to top", (db) => meterDeflection(-db)));
    });
  }

  /**
   * Makes the dock a drop target for sources dragged from the Routing page. `showing` is what the
   * dock shows now. Only a drag of sources is answered: any other drag passes over as before.
   */
  #dropTarget(store: ReturnType<typeof useStore>, showing: () => { surface: string } | { deviceId: string } | undefined, section: GaSection, hint: HTMLElement, ask: DropBar): void {
    /** What a drop of `drag` would do here: add to a mix, or be refused, with the words for either. */
    const verdict = (drag: SourceDrag): { add: { deviceId: string; mix: number }; text: string } | { add?: undefined; text: string } => {
      const target = showing();
      const name = (id: string) => {
        const entry = store.devices.peek().find((d) => d.id === id);
        return entry === undefined ? id : displayName(entry, store.workspace.peek());
      };
      if (!store.connected.peek()) return { text: "Not connected: nothing can be added" };
      if (target === undefined) return { text: "No device with a known mixer is shown here" };
      if ("surface" in target) return { text: "Showing a surface: set Show to This device to add channels" };
      if (target.deviceId !== drag.deviceId) return { text: `These sources are ${name(drag.deviceId)}'s; the dock shows ${name(target.deviceId)}` };
      const mix = store.channels(target.deviceId).meteredMix.peek();
      return { add: { deviceId: target.deviceId, mix }, text: dropHint(drag.sources.length, store.channels(target.deviceId).mixName(mix)) };
    };

    let depth = 0;
    let opening: ReturnType<typeof setTimeout> | undefined;
    const clear = () => {
      depth = 0;
      clearTimeout(opening);
      opening = undefined;
      this.removeAttribute("data-drop");
      hint.hidden = true;
    };
    const show = (drag: SourceDrag) => {
      const { add, text } = verdict(drag);
      this.setAttribute("data-drop", add === undefined ? "refused" : "add");
      hint.textContent = text;
      hint.hidden = false;
      return add;
    };
    const ours = (event: DragEvent) => (carriesSources(event.dataTransfer?.types) ? activeSourceDrag() : undefined);

    this.addEventListener("dragenter", (event) => {
      const drag = ours(event);
      if (drag === undefined) return;
      depth++;
      const add = show(drag);
      if (add !== undefined) event.preventDefault();
      // Held over a folded dock, the drag opens it, so the new channel is seen arriving.
      if (add !== undefined && section.collapsed && opening === undefined) opening = setTimeout(() => store.setMixerDockCollapsed(false), OPEN_ON_HOVER_MS);
    });
    this.addEventListener("dragover", (event) => {
      const drag = ours(event);
      if (drag === undefined) return;
      // Refused, the browser's own "no" cursor shows, and no drop comes.
      if (show(drag) === undefined) return;
      event.preventDefault();
      if (event.dataTransfer !== null) event.dataTransfer.dropEffect = "copy";
    });
    this.addEventListener("dragleave", (event) => {
      if (ours(event) === undefined) return;
      depth = Math.max(0, depth - 1);
      if (depth === 0) clear();
    });
    // A drag that ends anywhere else (Escape, or a drop elsewhere) takes the outline with it.
    const ended = () => clear();
    document.addEventListener("dragend", ended);
    document.addEventListener("drop", ended);
    this.onDisconnect(() => {
      document.removeEventListener("dragend", ended);
      document.removeEventListener("drop", ended);
    });

    // The question a drop that would double an input waits behind.
    let pending: { deviceId: string; mix: number; sources: SourceDrag["sources"] } | undefined;
    let waiting: ReturnType<typeof setTimeout> | undefined;
    const dismiss = () => {
      clearTimeout(waiting);
      waiting = undefined;
      pending = undefined;
      ask.bar.hidden = true;
    };
    this.onDisconnect(dismiss);
    ask.cancel.addEventListener("click", dismiss);
    ask.confirm.addEventListener("click", () => {
      const go = pending;
      dismiss();
      // Only while the dock still shows that mix: it was that mix the question was about.
      const target = showing();
      if (go === undefined || target === undefined || !("deviceId" in target) || target.deviceId !== go.deviceId || store.channels(go.deviceId).meteredMix.peek() !== go.mix) return;
      void addDroppedSources(store, go.deviceId, go.mix, go.sources);
    });

    this.addEventListener("drop", (event) => {
      const data = event.dataTransfer?.getData(SOURCE_MIME);
      if (data === undefined || data === "") return;
      event.preventDefault();
      clear();
      const drag = decodeSourceDrag(data);
      if (drag === undefined) return;
      const { add } = verdict(drag);
      if (add === undefined) return;
      // A folded dock opens, so what the drop did can be seen.
      if (section.collapsed) store.setMixerDockCollapsed(false);
      dismiss();
      void (async () => {
        await layoutForDrop(store, add.deviceId);
        const doubled = doublingsOf(store, add.deviceId, add.mix, drag.sources);
        if (doubled.length === 0) {
          await addDroppedSources(store, add.deviceId, add.mix, drag.sources);
          return;
        }
        const mixName = store.channels(add.deviceId).mixName(add.mix);
        pending = { deviceId: add.deviceId, mix: add.mix, sources: drag.sources };
        ask.reason.textContent = `${mixName}: ${doubled.join(" ")}`;
        ask.reason.title = ask.reason.textContent;
        ask.confirm.title = `${ask.reason.textContent}. Confirm to add anyway`;
        ask.bar.hidden = false;
        waiting = setTimeout(dismiss, DROP_CONFIRM_MS);
      })();
    });
  }
}

/** The question a drop that would double an input waits behind. */
interface DropBar {
  bar: HTMLElement;
  reason: HTMLElement;
  confirm: HTMLButtonElement;
  cancel: HTMLButtonElement;
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-mixer-dock": GaMixerDock;
  }
}
