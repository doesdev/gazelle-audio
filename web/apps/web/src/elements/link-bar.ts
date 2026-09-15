// Link badges and the link bar (decision P51), shared by the Inputs and Mixer pages. A badge starts
// a draft, or opens its link as one. While a draft is open, badges of the same kind add or remove
// channels, on this device or another picked from the device menu (the draft outlives a page's
// re-render), and Save makes the link.

import { h } from "../core/dom.ts";
import { signal } from "../core/signal.ts";
import type { LinkMode } from "../store/links.ts";
import type { ChannelRef, LinkKind } from "../store/store.ts";
import { useStore } from "./element.ts";

type Watch = (fn: () => void) => void;

interface LinkDraft {
  kind: LinkKind;
  /** The link being edited, if not a new one. */
  editing?: string;
  mode: LinkMode;
  members: ChannelRef[];
}

const draft = signal<LinkDraft | undefined>(undefined);

const KIND_NAMES: Record<LinkKind, string> = { preamp: "Preamp", line: "Line", adat: "ADAT", spdif: "S/PDIF", mixer: "Channel" };

export const LINK_STYLES = `
  .link[aria-pressed="true"] { background: var(--ga-accent); color: var(--ga-accent-text); }
  .link[data-drafting] { outline: 1px dashed var(--ga-accent); outline-offset: 1px; }
  .link-bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 6px 8px; border: 1px solid var(--ga-accent); border-radius: 3px; background: var(--ga-surface-raised); font-size: 12px; }
  .link-bar[hidden], .link-bar [hidden] { display: none; }
  .link-bar .members { flex: 1; min-width: 0; }
  .link-bar .modes { display: flex; }
  .link-bar .modes button { min-width: 64px; min-height: 22px; padding: 0 8px; border-radius: 0; font-size: 11px; font-weight: 600; }
  .link-bar .modes button + button { margin-left: -1px; }
  .link-bar .modes button:first-child { border-radius: 3px 0 0 3px; }
  .link-bar .modes button:last-child { border-radius: 0 3px 3px 0; }
  .link-bar .modes button[aria-pressed="true"] { position: relative; border-color: var(--ga-accent); background: var(--ga-accent); color: var(--ga-accent-text); }
`;

const isRef = (m: ChannelRef, deviceId: string, channel: number) => m.device_id === deviceId && m.channel === channel;

/** A channel's name, with its device's when that is not the device shown. */
function memberName(kind: LinkKind, member: ChannelRef, shown: string): string {
  const store = useStore();
  const device = store.devices.peek().find((d) => d.id === member.device_id);
  let name = `${KIND_NAMES[kind]} ${member.channel + 1}`;
  if (kind === "mixer" && device !== undefined && device.family !== null) {
    const own = store.channels(member.device_id).layout.peek().channels.find((c) => c.slot === member.channel)?.name;
    if (own !== undefined && own !== "") name = own;
  }
  return member.device_id === shown ? name : `${device?.model ?? member.device_id} ${name}`;
}

function saveDraft(d: LinkDraft): void {
  const store = useStore();
  try {
    const old = d.editing === undefined ? undefined : store.links.links.peek().find((l) => l.id === d.editing);
    for (const m of old?.members ?? []) {
      if (!d.members.some((n) => isRef(n, m.device_id, m.channel))) store.links.removeMember(d.kind, m.device_id, m.channel);
    }
    store.links.create(d.kind, d.members, d.mode);
    draft.value = undefined;
  } catch (error) {
    store.reportError(error instanceof Error ? error.message : String(error));
  }
}

/** The bar that shows the open draft: its members, the mode, Save, Unlink and Cancel. Hidden without a draft. */
export function linkBar(watch: Watch, deviceId: string): HTMLElement {
  const store = useStore();
  const members = h("span", { class: "members" });
  const setMode = (mode: LinkMode) => {
    const d = draft.peek();
    if (d !== undefined) draft.value = { ...d, mode };
  };
  const modes = (["absolute", "relative"] as const).map((mode) =>
    h("button", { type: "button", "data-testid": `link-mode-${mode}`, title: mode === "absolute" ? "Every member takes the same value" : "Every member moves by the same step, keeping its offset", "on:click": () => setMode(mode) }, mode === "absolute" ? "Same value" : "Relative"),
  );
  const save = h("button", { type: "button", "data-testid": "link-save", "on:click": () => draft.peek() && saveDraft(draft.peek() as LinkDraft) }, "Save");
  const unlink = h(
    "button",
    {
      type: "button",
      "data-testid": "link-unlink",
      "on:click": () => {
        const d = draft.peek();
        if (d?.editing !== undefined) store.links.remove(d.editing);
        draft.value = undefined;
      },
    },
    "Unlink",
  );
  const cancel = h("button", { type: "button", "data-testid": "link-cancel", "on:click": () => (draft.value = undefined) }, "Cancel");
  const bar = h("div", { class: "link-bar", "data-testid": "link-bar", role: "group", "aria-label": "Link" }, members, h("div", { class: "modes", role: "group", "aria-label": "Link mode" }, modes), save, unlink, cancel);
  watch(() => {
    const d = draft.value;
    bar.hidden = d === undefined;
    if (d === undefined) return;
    void store.devices.value;
    const names = d.members.map((m) => memberName(d.kind, m, deviceId));
    const noun = d.kind === "mixer" ? "channel" : `${KIND_NAMES[d.kind].toLowerCase()} input`;
    members.textContent = `${d.editing === undefined ? "New link" : "Link"}: ${names.join(", ")}${d.members.length < 2 ? ` (pick another ${noun}, on any device)` : ""}`;
    for (const [n, button] of modes.entries()) button.setAttribute("aria-pressed", String((n === 0 ? "absolute" : "relative") === d.mode));
    save.disabled = d.members.length < 2;
    unlink.hidden = d.editing === undefined;
  });
  return bar;
}

/** A channel's link badge: pressed when linked, numbered by its link, and the way into the link bar. */
export function linkButton(watch: Watch, kind: LinkKind, deviceId: string, channel: number, testId: string, className = "link"): HTMLButtonElement {
  const store = useStore();
  const self = { device_id: deviceId, channel };
  const button = h(
    "button",
    {
      type: "button",
      class: className,
      "data-control": "",
      "data-testid": testId,
      "on:click": () => {
        const d = draft.peek();
        if (d !== undefined && d.kind === kind) {
          const members = d.members.some((m) => isRef(m, deviceId, channel)) ? d.members.filter((m) => !isRef(m, deviceId, channel)) : [...d.members, self];
          draft.value = { ...d, members };
          return;
        }
        const link = store.links.linkOf(kind, deviceId, channel);
        draft.value = link === undefined ? { kind, mode: "absolute", members: [self] } : { kind, editing: link.id, mode: link.mode, members: [...link.members] };
      },
    },
    "⇆",
  );
  watch(() => {
    const d = draft.value;
    const links = store.links.links.value.filter((l) => l.kind === kind);
    const index = links.findIndex((l) => l.members.some((m) => isRef(m, deviceId, channel)));
    const link = links[index];
    const drafting = d !== undefined && d.kind === kind;
    const inDraft = drafting && d.members.some((m) => isRef(m, deviceId, channel));
    button.toggleAttribute("data-drafting", inDraft);
    button.setAttribute("aria-pressed", String(drafting ? inDraft : link !== undefined));
    button.textContent = link === undefined ? "⇆" : `⇆${index + 1}`;
    const name = memberName(kind, self, deviceId);
    const others = link?.members.filter((m) => !isRef(m, deviceId, channel)).map((m) => memberName(kind, m, deviceId)) ?? [];
    button.title = link === undefined ? `Link ${name} with others` : `${name} is linked with ${others.join(", ")}${link.mode === "relative" ? " (relative)" : ""}`;
    button.setAttribute("aria-label", drafting ? `${inDraft ? "Remove" : "Add"} ${name} ${inDraft ? "from" : "to"} the link` : button.title);
  });
  return button;
}
