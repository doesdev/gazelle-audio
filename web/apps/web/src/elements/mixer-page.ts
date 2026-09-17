// <ga-mixer device-id="…">: one device's mixer, built from the channels the user made (plan
// 2026-09-16). Channels scroll horizontally, followed by a "+" to add one, and the selected mix's
// master sits on the right. A device with no layout imports one from its routing when the page
// opens. The Mix menu in the bar selects the mix every strip shows, moves and meters (the user,
// 2026-09-16); it is remembered per device and carried in the address. In dry run it shows the bytes of the last
// command sent. The notes, the channels' scroll and a half-typed layout name are kept for the tab.

import { h } from "../core/dom.ts";
import { untracked } from "../core/signal.ts";
import { meterDeflection } from "../store/mixer.ts";
import { PROFILES } from "../store/profiles.ts";
import { STRIP_WIDTH_MAX, STRIP_WIDTH_MIN } from "../store/preferences.ts";
import { meterGradient } from "../themes/theme.ts";
import { GaElement, sheet, useStore } from "./element.ts";
import { LINK_STYLES, linkBar } from "./link-bar.ts";
// Masters are <ga-mix-master>; channels <ga-channel>, whose shadow heads are measured below.
import { replaceRoute } from "./router.ts";
import { keepOpen, keepScroll } from "./view-state.ts";

export class GaMixer extends GaElement {
  static override styles = [
    sheet(`
      :host { display: flex; flex-direction: column; gap: 8px; flex: 1; min-height: 0; }
      /* Channel width: a caption, an Auto | Fixed segmented control and an inset px field, all one height. */
      .width { display: flex; align-items: center; gap: 6px; }
      .caption { font-size: 11px; color: var(--ga-text-secondary); }
      .segmented { display: flex; }
      .segmented button {
        min-height: 26px;
        padding: 0 10px;
        border-radius: 0;
        color: var(--ga-text-secondary);
        font-size: 11px;
        font-weight: 600;
      }
      .segmented button + button { margin-left: -1px; }
      .segmented button:first-child { border-radius: 3px 0 0 3px; }
      .segmented button:last-child { border-radius: 0 3px 3px 0; }
      .segmented button[aria-pressed="true"] { position: relative; border-color: var(--ga-accent); background: var(--ga-accent); color: var(--ga-accent-text); }
      .px-field {
        display: flex;
        align-items: center;
        height: 26px;
        padding-right: 7px;
        border: 1px solid var(--ga-border-subtle);
        border-radius: 3px;
        background: var(--ga-surface-inset);
        cursor: text;
      }
      .px-field:focus-within { outline: 2px solid var(--ga-focus); outline-offset: 1px; }
      .px-field:has(input:disabled) { opacity: 0.5; cursor: not-allowed; }
      .px-field input {
        width: 3.4em;
        min-height: 24px;
        padding: 0 2px 0 6px;
        border: 0;
        background: transparent;
        text-align: right;
        font-variant-numeric: tabular-nums;
        appearance: textfield;
      }
      .px-field input:disabled { background: transparent; }
      .px-field input:focus-visible { outline: none; }
      .px-field input::-webkit-inner-spin-button, .px-field input::-webkit-outer-spin-button { appearance: none; margin: 0; }
      .px-field .unit { font-size: 11px; color: var(--ga-text-muted); }
      .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
      .bar select { min-height: 26px; }
      .spacer { flex: 1; }
      .notes { font-size: 11px; color: var(--ga-text-muted); }
      .notes summary { cursor: pointer; width: fit-content; }
      .notes ul { display: grid; gap: 2px; margin: 4px 0 0; padding: 0; list-style: none; }
      /* The bytes can be long (set_routing is 128 hex digits): one line, cut with an ellipsis, all of it in the tooltip. */
      .last-sent { display: flex; min-width: 0; max-width: 100%; font-size: 11px; white-space: nowrap; }
      .last-sent code { min-width: 0; overflow: hidden; text-overflow: ellipsis; font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 11px; }
      .drop { position: absolute; top: 4px; bottom: 4px; z-index: 2; width: 2px; background: var(--ga-accent); pointer-events: none; }
      .strips {
        position: relative;
        display: flex;
        flex: 1;
        gap: 2px;
        min-height: 0;
        /* No right padding: the sticky masters sit flush with the edge, so no strip shows past them.
           The top padding holds group bands, reserved for every channel so faders stay level. */
        padding: 28px 0 4px 4px;
        overflow-x: auto;
        overflow-y: hidden;
        border-radius: 3px;
        background: var(--ga-surface-inset);
      }
      /* Auto: channels share the row between the limits, and scroll once they reach the floor. Fixed: every channel is --strip-width. */
      .strips ga-channel { flex: 1 1 0; min-width: var(--strip-width-min); max-width: var(--strip-width-max); }
      .strips.fixed ga-channel { flex: 0 0 var(--strip-width); min-width: 0; max-width: none; }
      /* A group grows like its channels together: n channels' share, limits and gaps. */
      ga-channel-group {
        flex: var(--members) var(--members) 0;
        min-width: calc(var(--members) * var(--strip-width-min) + (var(--members) - 1) * 2px);
        max-width: calc(var(--members) * var(--strip-width-max) + (var(--members) - 1) * 2px);
      }
      .strips.fixed ga-channel-group { flex: 0 0 auto; min-width: 0; max-width: none; }
      ga-channel-group[collapsed] { flex: 0 0 28px; min-width: 28px; max-width: 28px; }
      .add {
        flex: 0 0 36px;
        min-height: 0;
        border: 1px dashed var(--ga-border-strong);
        background: transparent;
        color: var(--ga-text-secondary);
        font-size: 20px;
      }
      .add:hover:not(:disabled) { color: var(--ga-text-primary); }
      .masters {
        position: sticky;
        right: 0;
        z-index: 1;
        display: flex;
        gap: 2px;
        margin: -28px 0 -4px auto;
        padding: 28px 4px 4px 6px;
        background: var(--ga-surface-inset);
        box-shadow: -8px 0 8px -4px rgb(0 0 0 / 0.5);
      }
      .masters:empty { display: none; }
      .masters ga-mix-master { flex: 0 0 96px; }
      .starts { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
      .starts:empty { display: none; }
      .starts select, .starts button { min-height: 26px; }
      ${LINK_STYLES}
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const deviceId = this.getAttribute("device-id") ?? "";
    const topology = store.topology(deviceId);
    if (topology === undefined) {
      this.root.replaceChildren(h("p", { class: "placeholder" }, `The device ${deviceId} is not connected or is of an unknown model, so it has no mixer.`));
      return;
    }
    const channels = store.channels(deviceId);

    const metered = h("select", {
      "aria-label": "Mix",
      "data-testid": "mix-select",
      "on:change": () => {
        channels.meteredMix.value = Number(metered.value);
        replaceRoute({ page: "mixer", id: deviceId, sub: metered.value });
      },
    });
    const lastSent = h("span", { class: "last-sent muted", "data-testid": "last-sent" });
    const mixer0 = store.mixer(deviceId, 0);
    // Every mix's strips and links are read (sends show other mixes' levels), once: coming back to
    // the page uses what the store has. They are read again once the connection or the device has
    // come back (P80): at once while the page is open, else when it next opens.
    this.watch(() => {
      if (store.mixesToRead(deviceId)) untracked(() => void store.readMixes(deviceId));
    });
    const levelsNote = h("li", {}, "The device's mixer levels have not been read (as in dry run), so controls start at defaults and send when changed.");
    this.watch(() => {
      levelsNote.hidden = mixer0.stateKnown.value;
    });
    // One line by default, so the channels keep the height.
    const notes = h(
      "details",
      { class: "notes" },
      h("summary", {}, "How this mixer works"),
      h(
        "ul",
        {},
        h("li", {}, "A channel works once it has an input and a main mix. Its fader sets its level in the main mix; sends set its level in other mixes."),
        levelsNote,
      ),
    );
    keepOpen(notes, store.view("mixer:notes-open", false));
    const layoutName = store.view(`mixer:${deviceId}:layout-name`, "");
    const startFrom = store.view<string | undefined>(`mixer:${deviceId}:start-from`, undefined);

    // While no channel is set up, the mixer can start from a starting layout or one the user saved
    // (value "saved:<id>"); once channels are set up, the layout can be saved by name.
    const starts = h("div", { class: "starts" });
    const failed = (error: unknown) => store.reportError(error instanceof Error ? error.message : String(error));
    this.watch(() => {
      if (store.workspace.value === undefined) {
        starts.replaceChildren();
        return;
      }
      const setUp = channels.layout.value.channels.some((c) => channels.isActive(c));
      const saved = channels.savedLayouts();
      if (setUp) {
        // This row is rebuilt as the channels change, so the name typed so far is kept outside it.
        const name = h("input", { type: "text", value: layoutName.peek(), placeholder: "Layout name", "aria-label": "Name for the saved layout", "data-testid": "layout-save-name", "on:input": () => (layoutName.value = name.value) });
        const save = h(
          "button",
          {
            type: "button",
            "data-testid": "layout-save",
            title: "Save these channels, groups and mix names as a layout any device of this model can start from",
            "on:click": () => {
              try {
                channels.saveLayout(name.value);
                layoutName.value = "";
              } catch (error) {
                failed(error);
              }
            },
          },
          "Save layout",
        );
        starts.replaceChildren(h("span", { class: "caption" }, "Save as"), name, save);
        return;
      }
      const choices = topology.family === "quadro" || topology.family === "studio" ? PROFILES[topology.family] : [];
      const select = h(
        "select",
        { "aria-label": "Starting layout", "data-testid": "profile-select" },
        h("optgroup", { label: "Starting layouts" }, choices.map((p) => h("option", { value: p.id, title: p.description }, p.name))),
        saved.length > 0 ? h("optgroup", { label: "Saved" }, saved.map((l) => h("option", { value: `saved:${l.id}` }, l.name))) : undefined,
      );
      const chosenSaved = () => (select.value.startsWith("saved:") ? select.value.slice("saved:".length) : undefined);
      const apply = h(
        "button",
        {
          type: "button",
          "data-testid": "profile-apply",
          title: "Replace these channels with the chosen layout and route it",
          "on:click": () => {
            const id = chosenSaved();
            (id === undefined ? channels.applyProfile(select.value) : channels.applySavedLayout(id)).catch(failed);
          },
        },
        "Apply",
      );
      const remove = h(
        "button",
        {
          type: "button",
          "data-testid": "layout-remove",
          title: "Delete the chosen saved layout",
          "on:click": () => {
            const id = chosenSaved();
            if (id !== undefined) channels.removeSavedLayout(id);
          },
        },
        "Delete",
      );
      const syncRemove = () => (remove.hidden = chosenSaved() === undefined);
      const chosen = startFrom.peek();
      if (chosen !== undefined && [...select.options].some((o) => o.value === chosen)) select.value = chosen;
      select.addEventListener("change", () => (startFrom.value = select.value));
      select.addEventListener("change", syncRemove);
      syncRemove();
      starts.replaceChildren(h("span", { class: "caption" }, "Start from"), select, apply, remove);
    });

    const strips = h("div", { class: "strips" });
    const add = h("button", { type: "button", class: "add", title: "Add a channel", "aria-label": "Add a channel", "data-testid": "add-channel", "on:click": () => channels.add() }, "+");
    const masters = h("div", { class: "masters", "aria-label": "Mix masters" });

    const stripWidth = h("input", { type: "number", min: STRIP_WIDTH_MIN, max: STRIP_WIDTH_MAX, step: 1, "aria-label": "Channel width in px", "data-testid": "strip-width" });
    const autoWidth = h("button", { type: "button", title: "Fit channels to the window", "data-testid": "strip-width-auto", "on:click": () => store.setMixerWidth({ auto: true }) }, "Auto");
    const fixedWidth = h(
      "button",
      {
        type: "button",
        title: "Set a channel width; channels scroll when they do not fit",
        "data-testid": "strip-width-fixed",
        "on:click": () => {
          store.setMixerWidth({ auto: false });
          stripWidth.focus();
          stripWidth.select();
        },
      },
      "Fixed",
    );
    const commitWidth = () => {
      const px = Number(stripWidth.value);
      if (stripWidth.value.trim() !== "" && Number.isFinite(px)) store.setMixerWidth({ px });
      stripWidth.value = String(store.mixerWidth.peek().px);
    };
    stripWidth.addEventListener("change", commitWidth);
    stripWidth.addEventListener("keydown", (event) => {
      if (event.key === "Enter") commitWidth();
    });
    const width = h(
      "div",
      { class: "width", role: "group", "aria-label": "Channel width" },
      h("span", { class: "caption", "aria-hidden": "true" }, "Width"),
      h("div", { class: "segmented" }, autoWidth, fixedWidth),
      h("label", { class: "px-field" }, stripWidth, h("span", { class: "unit" }, "px")),
    );
    strips.style.setProperty("--strip-width-min", `${STRIP_WIDTH_MIN}px`);
    strips.style.setProperty("--strip-width-max", `${STRIP_WIDTH_MAX}px`);

    this.root.replaceChildren(
      h("div", { class: "bar" }, h("label", { class: "width" }, h("span", { class: "caption" }, "Mix"), metered), h("span", { class: "spacer" }), width, lastSent),
      linkBar((fn) => this.watch(fn), deviceId),
      starts,
      notes,
      strips,
    );

    // A device without a layout imports one from its routing, once the workspace has loaded.
    let imported = false;
    this.watch(() => {
      if (store.workspace.value === undefined || imported || channels.configured) return;
      imported = true;
      void channels.importFromDevice();
    });

    // Point the device's meters at the chosen mix.
    this.watch(() => store.mixer(deviceId, channels.meteredMix.value).activate());

    // Channels in layout order (consecutive channels of one group inside a group element), then
    // "+", then the masters of the mixes in use.
    const headHeights = new Map<Element, number>();
    const heads = new ResizeObserver((entries) => {
      for (const entry of entries) headHeights.set(entry.target, entry.borderBoxSize[0]?.blockSize ?? entry.contentRect.height);
      const tallest = Math.max(0, ...headHeights.values());
      if (tallest > 0) masters.style.setProperty("--channel-head", `${tallest}px`);
    });
    this.onDisconnect(() => heads.disconnect());
    // Where each mix plays is read once, as the mixes are, and again once the connection or the
    // device has come back.
    const outputGroups = [...new Set(channels.outputPairs().map((pair) => pair.destination))];
    this.watch(() => {
      if (store.routesToRead(deviceId, outputGroups)) untracked(() => void store.readRoutes(deviceId, outputGroups));
    });

    // Drag to move: a channel's grip starts it; where it is dropped among the other channels sets its
    // place and group (ChannelsModel.place: between two members of a group it joins, elsewhere none).
    const indicator = h("div", { class: "drop", hidden: true });
    let dragging: { id: string; pointer: number; element: HTMLElement } | undefined;
    const dropAt = (x: number, id: string) => {
      const others = [...strips.querySelectorAll("ga-channel")].filter((e) => e.getAttribute("channel-id") !== id && e.getBoundingClientRect().width > 0);
      const index = others.findIndex((e) => {
        const r = e.getBoundingClientRect();
        return x < r.left + r.width / 2;
      });
      return { index: index < 0 ? others.length : index, others };
    };
    strips.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || !store.connected.peek()) return;
      const path = event.composedPath();
      if (!path.some((n) => n instanceof HTMLElement && n.hasAttribute("data-grip"))) return;
      const element = path.find((n): n is HTMLElement => n instanceof HTMLElement && n.localName === "ga-channel");
      const id = element?.getAttribute("channel-id");
      if (element === undefined || id === null || id === undefined) return;
      dragging = { id, pointer: event.pointerId, element };
      strips.setPointerCapture(event.pointerId);
      element.setAttribute("data-dragging", "");
      event.preventDefault();
    });
    strips.addEventListener("pointermove", (event) => {
      if (dragging?.pointer !== event.pointerId) return;
      const { index, others } = dropAt(event.clientX, dragging.id);
      const row = strips.getBoundingClientRect();
      const target = others[index];
      const edge = target !== undefined ? target.getBoundingClientRect().left - 1 : (others.at(-1)?.getBoundingClientRect().right ?? row.left) + 1;
      indicator.style.left = `${edge - row.left + strips.scrollLeft}px`;
      indicator.hidden = false;
    });
    const finish = (event: PointerEvent, drop: boolean) => {
      if (dragging?.pointer !== event.pointerId) return;
      const { id, element } = dragging;
      dragging = undefined;
      element.removeAttribute("data-dragging");
      indicator.hidden = true;
      if (drop) channels.place(id, dropAt(event.clientX, id).index);
    };
    strips.addEventListener("pointerup", (event) => finish(event, true));
    strips.addEventListener("pointercancel", (event) => finish(event, false));

    const elements = new Map<string, HTMLElement>();
    const groupElements = new Map<string, HTMLElement>();
    let structure = "";
    let mastersKey = "";
    this.watch(() => {
      const layout = channels.layout.value;
      const list = layout.channels;
      for (const id of [...elements.keys()]) if (!list.some((c) => c.id === id)) elements.delete(id);
      const channelElement = (id: string, slot: number) => {
        let element = elements.get(id);
        if (element === undefined) {
          element = h("ga-channel", { "device-id": deviceId, "channel-id": id, "data-channel-slot": String(slot) });
          elements.set(id, element);
        }
        return element;
      };
      const runs: { group: string | undefined; members: { id: string; slot: number }[] }[] = [];
      for (const c of list) {
        const group = c.group !== undefined && layout.groups.some((g) => g.id === c.group) ? c.group : undefined;
        const last = runs.at(-1);
        if (group !== undefined && last?.group === group) last.members.push({ id: c.id, slot: c.slot });
        else runs.push({ group, members: [{ id: c.id, slot: c.slot }] });
      }
      const runsKey = runs.map((run) => `${run.group ?? "-"}:${run.members.map((m) => m.id).join(",")}`).join("|");
      if (runsKey !== structure) {
        structure = runsKey;
        const seen = new Map<string, number>();
        const kept = new Set<string>();
        const children = runs.map((run) => {
          if (run.group === undefined) return run.members.map((m) => channelElement(m.id, m.slot));
          // A group split by moving a channel on its own gets one element per run.
          const count = seen.get(run.group) ?? 0;
          seen.set(run.group, count + 1);
          const groupKey = `${run.group}#${count}`;
          kept.add(groupKey);
          let element = groupElements.get(groupKey);
          if (element === undefined) {
            element = h("ga-channel-group", { "device-id": deviceId, "group-id": run.group });
            groupElements.set(groupKey, element);
          }
          element.style.setProperty("--members", String(run.members.length));
          element.replaceChildren(...run.members.map((m) => channelElement(m.id, m.slot)));
          return element;
        });
        for (const groupKey of [...groupElements.keys()]) if (!kept.has(groupKey)) groupElements.delete(groupKey);
        strips.replaceChildren(...children.flat(), add, masters, indicator);
      }
      add.disabled = list.length >= 32 - channels.firstSlot;

      // The strips are the selected mix, so its master is the one beside them.
      const key = String(channels.meteredMix.value);
      if (key !== mastersKey) {
        mastersKey = key;
        masters.replaceChildren(h("ga-mix-master", { "device-id": deviceId, mix: key }));
      }
      // Masters' heads take the channel heads' height, so every fader starts level.
      heads.disconnect();
      headHeights.clear();
      for (const channel of strips.querySelectorAll("ga-channel")) {
        const channelHead = (channel as GaElement).root.querySelector(".head");
        if (channelHead !== null) heads.observe(channelHead);
      }
    });

    // After the channels are in, so there is something to scroll back to.
    this.onDisconnect(keepScroll(strips, store.view(`mixer:${deviceId}:scroll`, 0), "left"));

    this.watch(() => {
      const { auto, px } = store.mixerWidth.value;
      autoWidth.setAttribute("aria-pressed", String(auto));
      fixedWidth.setAttribute("aria-pressed", String(!auto));
      stripWidth.disabled = auto;
      if (this.root.activeElement !== stripWidth) stripWidth.value = String(px);
      strips.classList.toggle("fixed", !auto);
      strips.style.setProperty("--strip-width", `${px}px`);
    });
    this.watch(() => {
      metered.replaceChildren(...Array.from({ length: channels.mixCount }, (_, mix) => h("option", { value: String(mix) }, channels.mixName(mix))));
      metered.value = String(channels.meteredMix.value);
    });
    this.watch(() => {
      add.toggleAttribute("data-offline", !store.connected.value);
    });
    this.watch(() => {
      // Meter gradient stops are dBFS; place them on the same scale the meters use.
      this.style.setProperty("--mixer-meter-gradient", meterGradient(store.theme.value.meter.gradient, undefined, "to top", (db) => meterDeflection(-db)));
    });
    this.watch(() => {
      const sent = store.lastSent.value;
      const dryRun = store.server.value.dry_run;
      if (sent === undefined || sent.deviceId !== deviceId) {
        lastSent.textContent = dryRun ? "Dry run: nothing is written to the device" : "";
        return;
      }
      lastSent.replaceChildren(`${sent.dryRun ? "Dry run, would send" : "Sent"} ${sent.command}: `, h("code", { title: sent.hex }, sent.hex));
    });
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-mixer": GaMixer;
  }
}
