// <ga-recording>: the Recording page. Records the aggregate's inputs, channels from both interfaces
// lined up as a DAW would get them, to one Broadcast WAV per channel, with a pre-roll held in memory
// while armed so a take can start a little before Record was pressed.
//
// Top to bottom:
// - **Transport** (`recording-transport.ts`): the preset, the state, the pre-roll, and Arm, Record,
//   Stop and Disarm. Space presses Record while armed and Stop while recording, whenever the page is
//   shown and a field or button does not have the focus (nothing else in the app uses Space).
// - **Channels**: what is being recorded, each named as the aggregate names it, with its level.
// - **Presets**: the list and the editor. A preset holds the channels (by interface and input, as
//   the Aggregate page names them), the folder, the file names, 24-bit or 32-bit float, and how much
//   memory the pre-roll takes. Presets live in the workspace. On a phone they are shown but not
//   edited: a preset names a folder on the computer.
// - **Takes**: the takes recorded since Gazelle started, newest first, with their folder and files.

import { h } from "../core/dom.ts";
import { signal, untracked } from "../core/signal.ts";
import { fileOf, folderOf, newPreset, PATTERN_DEFAULT, PERCENT_DEFAULT, PERCENTS, PRESET_KEY, presetToOffer, recordingModel, takeLine, type RecordingPreset } from "../store/recording.ts";
import type { Store } from "../store/store.ts";
import { bindConfirm } from "./controls.ts";
import { commitOnEnter, GaElement, sheet, useStore } from "./element.ts";
import { CHANNEL_STYLES, channelMeters, recordingTransport, remembered, spaceToggles, TRANSPORT_STYLES, type TransportHost } from "./recording-transport.ts";

/** What auto-arm means for the drivers, and for this PC's other work, said beside the setting. */
export const AUTO_ARM_HELP =
  "Gazelle arms with this preset whenever it starts, and again when the interfaces drop out and come back. Always armed means Gazelle always holds the audio drivers of both interfaces, so a DAW may not be able to use them: whether the Antelope drivers let a second program in at the same time has not been tried yet. Disarming by hand pauses auto-arm until Gazelle next starts or you arm again, so a DAW or a measurement can have the interfaces. Quitting Gazelle, or restarting it to update, finishes any take first. Ending it by force cannot: the take's files still open, but lose up to its last two seconds.";

export class GaRecording extends GaElement {
  static override styles = [
    sheet(`
      :host { display: block; }
      .sections { display: grid; gap: 12px; max-width: 980px; }
      ${TRANSPORT_STYLES}
      ${CHANNEL_STYLES}
      .empty { margin: 0; font-size: 13px; color: var(--ga-text-muted); }
      .computer { display: grid; gap: 8px; }
      .computer .check { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; font-size: 13px; }
      .computer .note[hidden] { display: none; }
      .presets { display: grid; grid-template-columns: minmax(180px, 220px) minmax(0, 1fr); gap: 12px; }
      .list { display: grid; gap: 4px; align-content: start; }
      .list button { text-align: left; }
      .list button[aria-current] { border-color: var(--ga-accent); }
      .editor { display: grid; gap: 10px; }
      .editor[data-locked] .fields { opacity: 0.6; }
      .fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 8px 12px; }
      .fields label { display: grid; gap: 2px; font-size: 11px; color: var(--ga-text-secondary); }
      .fields input, .fields select { width: 100%; box-sizing: border-box; }
      .picker { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 8px; }
      .interface { display: grid; gap: 2px; align-content: start; padding: 6px 8px; border: 1px solid var(--ga-border-subtle); border-radius: 4px; }
      .interface h3 { margin: 0 0 4px; font-size: 13px; }
      .interface label { display: flex; align-items: center; gap: 6px; font-size: 12px; }
      .row { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
      .note { margin: 0; font-size: 12px; color: var(--ga-text-muted); }
      .takes { display: grid; gap: 8px; }
      .take { display: grid; gap: 2px; padding: 6px 8px; border-radius: 4px; background: var(--ga-surface-raised); font-size: 13px; }
      .take .where { font-family: ui-monospace, "Cascadia Mono", monospace; font-size: 12px; color: var(--ga-text-secondary); user-select: all; }
      .take .files { margin: 0; padding-left: 18px; font-size: 12px; color: var(--ga-text-secondary); }
      .take .problem { color: var(--ga-notice-warning); }
      @media (max-width: 700px) { .presets { grid-template-columns: 1fr; } .channel { grid-template-columns: minmax(0, 1fr) 90px 56px; } }
    `),
  ];

  protected override render(): void {
    const store = useStore();
    const host = { watch: (fn: () => void) => this.watch(fn), onDisconnect: (fn: () => void) => this.onDisconnect(fn) };
    const section = (id: string, heading: string, explain: string, ...content: Node[]) => h("ga-section", { heading, explain, "data-testid": `recording-section-${id}` }, ...content);

    const channels = channelMeters(host, "recording-channel", "Arm to see the channels being recorded, each with its level. Their names are the ones a DAW shows, and the ones their files take.");
    this.root.replaceChildren(
      h(
        "div",
        { class: "sections" },
        section("transport", "Transport", "recording.transport", recordingTransport(host, false)),
        section("channels", "Channels", "recording.channels", channels),
        // The windows and the settings are this computer's: a phone is not shown them.
        store.phone ? false : section("computer", "Widget, hub and auto-arm", "recording.computer", this.#computer(store, host)),
        section("presets", "Presets", "recording.presets", this.#presets(store)),
        section("takes", "Takes", "recording.takes", this.#takes(store)),
      ),
    );

    // Space: Record while armed, Stop while recording, unless something that takes Space has focus.
    spaceToggles(host);
  }

  /**
   * This computer as a studio: the recording widget and hub windows, auto-arm and starting in the
   * hub. Read when the page opens and whenever its window comes back into view, since the tray
   * changes the same things.
   */
  #computer(store: Store, host: TransportHost): HTMLElement {
    const model = recordingModel(store);
    const reread = () => void model.readComputer();
    reread();
    const onVisible = () => {
      if (document.visibilityState === "visible") reread();
    };
    document.addEventListener("visibilitychange", onVisible);
    addEventListener("focus", reread);
    this.onDisconnect(() => {
      document.removeEventListener("visibilitychange", onVisible);
      removeEventListener("focus", reread);
    });

    const widget = h("button", { type: "button", "data-testid": "recording-widget-toggle", "data-explain": "recording.widget-toggle" }, "Open the recording widget");
    widget.addEventListener("click", () => void model.setWindow("widget", { open: !(model.windows.peek()?.widget ?? false) }));
    const hub = h("button", { type: "button", "data-testid": "recording-hub-open", "data-explain": "recording.hub-open" }, "Open the recording hub");
    hub.addEventListener("click", () => void model.setWindow("hub", { open: true }));
    const noWindows = h("p", { class: "note", "data-testid": "recording-no-windows", hidden: true });

    const autoArm = h("input", { type: "checkbox", "data-testid": "recording-auto-arm", "data-explain": "recording.auto-arm" });
    const autoPreset = h("select", { "aria-label": "Preset auto-arm arms with", "data-testid": "recording-auto-arm-preset", "data-explain": "recording.auto-arm-preset" });
    autoArm.addEventListener("change", () => {
      const preset = autoPreset.value === "" ? null : autoPreset.value;
      void model.setSettings(autoArm.checked ? { auto_arm: true, auto_arm_preset: preset } : { auto_arm: false });
    });
    autoPreset.addEventListener("change", () => void model.setSettings({ auto_arm_preset: autoPreset.value === "" ? null : autoPreset.value }));
    const startInHub = h("input", { type: "checkbox", "data-testid": "recording-start-in-hub", "data-explain": "recording.start-in-hub" });
    startInHub.addEventListener("change", () => void model.setSettings({ start_in_hub: startInHub.checked }));

    host.watch(() => {
      const windows = model.windows.value;
      const settings = model.settings.value;
      const presets = store.workspace.value?.recording?.presets ?? [];
      const connected = store.connected.value;
      const available = windows?.available ?? false;
      widget.textContent = windows?.widget === true ? "Close the recording widget" : "Open the recording widget";
      widget.disabled = !connected || !available;
      hub.disabled = !connected || !available;
      startInHub.disabled = !connected || settings === undefined;
      noWindows.hidden = windows === undefined || available;
      noWindows.textContent = windows?.reason ?? "";

      autoPreset.replaceChildren(...presets.map((p) => h("option", { value: p.id }, p.name)));
      if (presets.length === 0) autoPreset.append(h("option", { value: "" }, "No presets yet"));
      const chosen = settings?.auto_arm_preset ?? untracked(() => presetToOffer(presets, model.status.peek(), remembered(PRESET_KEY))) ?? "";
      autoPreset.value = presets.some((p) => p.id === chosen) ? chosen : (presets[0]?.id ?? "");
      autoArm.checked = settings?.auto_arm ?? false;
      autoArm.disabled = !connected || settings === undefined || presets.length === 0;
      autoPreset.disabled = !connected || settings === undefined || presets.length === 0;
      startInHub.checked = settings?.start_in_hub ?? false;
    });

    return h(
      "div",
      { class: "computer" },
      h("div", { class: "row" }, widget, hub),
      noWindows,
      h("p", { class: "note" }, "The widget is a small window that stays on top of everything else, with the state, the time and the buttons; drag it anywhere by its body. The hub fills a screen, to be read from across the room: Space records and stops, Esc leaves full screen. Both open from the tray as well."),
      h("label", { class: "check" }, autoArm, "Auto-arm with ", autoPreset),
      h("p", { class: "note" }, AUTO_ARM_HELP),
      h("label", { class: "check" }, startInHub, "Start in the recording hub"),
      h("p", { class: "note" }, "When Gazelle starts, including at login with Start on boot, it opens the hub full screen on the monitor it was last on. The app's own window still starts in the tray at login."),
    );
  }

  /** The presets: a list, and the editor for the one chosen. */
  #presets(store: Store): HTMLElement {
    const model = recordingModel(store);
    const list = h("div", { class: "list", "data-testid": "recording-preset-list" });
    const editor = h("div", { class: "editor", "data-testid": "recording-preset-editor" });
    const selected = signal<string | undefined>(undefined);
    const add = h("button", { type: "button", "data-testid": "recording-preset-new", "data-explain": "recording.preset-new" }, "New preset");
    add.addEventListener("click", () => {
      const presets = store.workspace.peek()?.recording?.presets ?? [];
      const made = newPreset(presets);
      selected.value = made.id;
      store.editWorkspace((workspace) => ({ ...workspace, recording: { ...workspace.recording, presets: [...presets, made] } }));
    });
    const phoneNote = h("p", { class: "note", hidden: true }, "Presets are made and changed on the computer, since each one names a folder there. Choose one in the Transport to arm with it.");
    let built = "";
    this.watch(() => {
      const presets = store.workspace.value?.recording?.presets ?? [];
      const status = model.status.value;
      const phone = store.phone;
      phoneNote.hidden = !phone;
      add.hidden = phone;
      const wanted = selected.value;
      const current = wanted !== undefined && presets.some((p) => p.id === wanted) ? wanted : presets[0]?.id;
      const editing = presets.find((p) => p.id === current);
      const armedWith = status?.state === "off" ? undefined : status?.preset?.id;
      const key = JSON.stringify([presets, current, armedWith, phone, store.connected.value, store.devices.value.map((d) => d.id), store.workspace.value?.aggregate, store.workspace.value?.aliases]);
      if (key === built) return;
      built = key;
      untracked(() => {
        list.replaceChildren(
          ...presets.map((p) =>
            h(
              "button",
              {
                type: "button",
                "aria-current": p.id === current ? "true" : undefined,
                "data-testid": `recording-preset-${p.id}`,
                "data-explain": "recording.preset-item",
                "data-explain-name": p.name,
                "on:click": () => {
                  selected.value = p.id;
                },
              },
              p.id === armedWith ? `${p.name} (armed)` : p.name,
            ),
          ),
          add,
        );
        if (editing === undefined) {
          editor.replaceChildren(h("p", { class: "empty" }, phone ? "No presets have been made on the computer yet." : "No presets yet. A preset says which channels to record, where the files go and what they are called."));
          return;
        }
        editor.replaceChildren(...this.#editor(store, editing, editing.id === armedWith || phone));
        editor.toggleAttribute("data-locked", editing.id === armedWith);
      });
    });
    return h("div", {}, phoneNote, h("div", { class: "presets" }, list, editor));
  }

  #editor(store: Store, preset: RecordingPreset, locked: boolean): Node[] {
    const edit = (change: (p: RecordingPreset) => RecordingPreset) =>
      store.editWorkspace((workspace) => ({
        ...workspace,
        recording: { ...workspace.recording, presets: (workspace.recording?.presets ?? []).map((p) => (p.id === preset.id ? change({ ...p }) : p)) },
      }));
    const connected = store.connected.peek();
    const disabled = locked || !connected;
    const text = (testid: string, explain: string, value: string, placeholder: string, commit: (value: string) => void) => {
      const input = h("input", { type: "text", value, placeholder, "data-testid": testid, "data-explain": explain, disabled });
      commitOnEnter(input, commit, () => value);
      return input;
    };
    const name = text("recording-preset-name", "recording.preset-name", preset.name, "Preset name", (value) => {
      if (value.trim() !== "") edit((p) => ({ ...p, name: value.trim() }));
    });
    const folder = text("recording-preset-folder", "recording.preset-folder", preset.folder ?? "", untracked(() => recordingModel(store).status.peek()?.default_folder) ?? "Documents\\Gazelle Recordings", (value) =>
      edit((p) => {
        const next = { ...p };
        if (value.trim() === "") delete next.folder;
        else next.folder = value.trim();
        return next;
      }),
    );
    const pattern = text("recording-preset-pattern", "recording.preset-pattern", preset.pattern ?? "", PATTERN_DEFAULT, (value) =>
      edit((p) => {
        const next = { ...p };
        if (value.trim() === "") delete next.pattern;
        else next.pattern = value.trim();
        return next;
      }),
    );
    const format = h("select", { "data-testid": "recording-preset-format", "data-explain": "recording.preset-format", disabled }, h("option", { value: "int24" }, "24-bit integer"), h("option", { value: "float32" }, "32-bit float"));
    format.value = preset.format ?? "int24";
    format.addEventListener("change", () => edit((p) => ({ ...p, format: format.value === "float32" ? "float32" : "int24" })));
    const percent = h("select", { "data-testid": "recording-preset-percent", "data-explain": "recording.preset-percent", disabled }, ...PERCENTS.map((n) => h("option", { value: String(n) }, `${n} % of free memory`)));
    percent.value = String(preset.preroll_percent ?? PERCENT_DEFAULT);
    percent.addEventListener("change", () => edit((p) => ({ ...p, preroll_percent: Number(percent.value) })));
    const cap = h("input", { type: "number", min: 5, max: 3600, step: 1, value: preset.preroll_max_seconds === undefined ? "" : String(preset.preroll_max_seconds), placeholder: "As much as that gives", "data-testid": "recording-preset-cap", "data-explain": "recording.preset-cap", disabled });
    cap.addEventListener("change", () =>
      edit((p) => {
        const next = { ...p };
        const seconds = Number(cap.value);
        if (cap.value.trim() === "" || !Number.isFinite(seconds)) delete next.preroll_max_seconds;
        else next.preroll_max_seconds = Math.min(3600, Math.max(5, seconds));
        return next;
      }),
    );
    const remove = h("button", { type: "button", "data-testid": "recording-preset-delete", "data-explain": "recording.preset-delete", disabled }, "Delete preset");
    this.onDisconnect(
      bindConfirm(remove, "Delete preset", () =>
        store.editWorkspace((workspace) => ({ ...workspace, recording: { ...workspace.recording, presets: (workspace.recording?.presets ?? []).filter((p) => p.id !== preset.id) } })),
      ),
    );

    // The channels, by interface, as the Aggregate page names them.
    const { devices, inputs } = untracked(() => store.aggregateInputs());
    const chosen = new Set((preset.channels ?? []).map((c) => `${c.device}:${c.channel}`));
    const picker = h("div", { class: "picker", "data-testid": "recording-preset-channels" });
    if (inputs.length === 0) {
      picker.append(h("p", { class: "note" }, "The aggregate's interfaces are set up on the Aggregate page; their inputs are listed here once it knows them."));
    }
    devices.forEach((device, index) => {
      const own = inputs.filter((c) => c.index === index);
      if (own.length === 0) return;
      const boxes = own.map((c) => {
        const tick = h("input", { type: "checkbox", checked: chosen.has(`${index}:${c.channel}`), disabled, "data-testid": `recording-pick-${index}-${c.channel}`, "data-explain": "recording.preset-channel", "data-explain-name": c.text });
        tick.addEventListener("change", () =>
          edit((p) => {
            const rest = (p.channels ?? []).filter((one) => !(one.device === index && one.channel === c.channel));
            return { ...p, channels: tick.checked ? [...rest, { device: index, channel: c.channel }].sort((a, b) => a.device - b.device || a.channel - b.channel) : rest };
          }),
        );
        return h("label", {}, tick, c.text);
      });
      picker.append(h("div", { class: "interface" }, h("h3", {}, device), ...boxes));
    });

    const field = (label: string, control: HTMLElement) => h("label", {}, label, control);
    const parts: (Node | false)[] = [
      h("div", { class: "fields" }, field("Name", name), field("Format", format), field("Pre-roll memory", percent), field("Keep at most (seconds)", cap), field("Folder", folder), field("File names", pattern)),
      h("p", { class: "note" }, "File names can hold {channel} (the channel's name, as a DAW shows it), {take}, {date}, {time} and {preset}; {channel} and {take} are needed. A file is never overwritten: the take number moves on instead."),
      h("p", { class: "note" }, "The rate and buffer size are the aggregate's, from the Aggregate page. The pre-roll is sized when you arm, from the memory free then: at least 5 seconds, never more than half of the free memory or 4 GB."),
      picker,
      locked && !store.phone ? h("p", { class: "note" }, "This preset is armed, so it cannot be changed now. Disarm first.") : false,
      store.phone ? false : h("div", { class: "row" }, remove),
    ];
    return parts.filter((part): part is Node => part !== false);
  }

  /** The takes recorded since Gazelle started, with where they are. */
  #takes(store: Store): HTMLElement {
    const model = recordingModel(store);
    const box = h("div", { class: "takes", "data-testid": "recording-takes" });
    this.watch(() => {
      const takes = model.takes.value;
      if (takes.length === 0) {
        box.replaceChildren(h("p", { class: "empty" }, "No takes yet. Each take is one WAV file per channel, all starting at the same moment, with a log beside them."));
        return;
      }
      box.replaceChildren(
        ...takes.map((take) =>
          h(
            "div",
            { class: "take", "data-testid": `recording-take-${take.number}` },
            h("span", { class: "line", "data-explain": "recording.take" }, `${takeLine(take)} · ${take.preset}`),
            h("span", { class: "where", "data-explain": "recording.take-folder" }, take.folder === "" ? folderOf(take.files[0] ?? "") : take.folder),
            h("ul", { class: "files" }, ...take.files.map((file) => h("li", {}, fileOf(file))), h("li", {}, fileOf(take.log))),
            take.overruns + take.dropouts > 0 ? h("span", { class: "problem" }, `${take.overruns + take.dropouts} blocks were lost in this take; its log says where.`) : false,
            take.stopped_by === undefined ? false : h("span", { class: "problem" }, `Stopped by Gazelle: ${take.stopped_by}.`),
            take.problem === undefined ? false : h("span", { class: "problem" }, take.problem),
          ),
        ),
      );
    });
    return box;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ga-recording": GaRecording;
  }
}
