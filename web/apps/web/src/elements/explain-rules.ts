// The explain mode's rules, kept free of the DOM so they can be tested on their own: what an
// explanation says once the thing it is about has a name, which element along an event's path it
// is about, and where its panel goes so that it never covers that element.
//
// Elements carry a key, `data-explain="strip.fader"`, and may carry a name for it,
// `data-explain-name="Vox"`. The catalogue (`explain-catalogue.ts`, a chunk of its own) holds the
// text for each key; `{name}` in it is replaced by the element's name, or by the entry's own
// fallback ("this channel") when the element gives none.

/** One entry in the catalogue. */
export interface Explanation {
  /** A few words: what this is. */
  title: string;
  /** What it is, in a sentence or two. */
  what: string;
  /** What changing it does, to the sound or to the device. */
  effect?: string;
  /** Anything to watch for. */
  watch?: string;
  /** What `{name}` reads as when the element names nothing. */
  name?: string;
}

export type Catalogue = Readonly<Record<string, Explanation>>;

/** An explanation with its name filled in, as the panel shows it. */
export interface ResolvedExplanation {
  title: string;
  what: string;
  effect: string | undefined;
  watch: string | undefined;
}

/** Fills `{name}` in with the element's name, or the entry's fallback when there is none. */
export function resolveExplanation(entry: Explanation, name: string | undefined): ResolvedExplanation {
  const called = name !== undefined && name !== "" ? name : (entry.name ?? "");
  const fill = (text: string) => text.replaceAll("{name}", called);
  return { title: fill(entry.title), what: fill(entry.what), effect: entry.effect === undefined ? undefined : fill(entry.effect), watch: entry.watch === undefined ? undefined : fill(entry.watch) };
}

/**
 * Whether an element's own tooltip says something its explanation does not already: some sentence of
 * it that the explanation does not hold. A button's fixed title often repeats what the catalogue says;
 * a live one ("Sums Mix 1 to mono, so HP1 goes mono too") never does, and is worth its line.
 */
export function adds(own: string, text: ResolvedExplanation): boolean {
  const said = [text.title, text.what, text.effect ?? "", text.watch ?? ""].join(" ").toLowerCase();
  return own
    .split(/(?<=[.!?])\s+/)
    .map((sentence) => sentence.trim().replace(/[.!?]+$/, "").toLowerCase())
    .some((sentence) => sentence !== "" && !said.includes(sentence));
}

interface Attributed {
  getAttribute(name: string): string | null;
}

const attributed = (node: unknown): node is Attributed => typeof node === "object" && node !== null && typeof (node as { getAttribute?: unknown }).getAttribute === "function";

/**
 * The nearest element along an event's composed path (innermost first) that carries a key, and its
 * position in the path. A composed path runs through every shadow root on the way out, which is what
 * lets one listener on the document find a fader three shadow roots down.
 */
export function explainKeyIn(path: readonly unknown[]): { index: number; key: string; name: string | undefined } | undefined {
  for (const [index, node] of path.entries()) {
    if (!attributed(node)) continue;
    const key = node.getAttribute("data-explain");
    if (key !== null && key !== "") return { index, key, name: node.getAttribute("data-explain-name") ?? undefined };
  }
  return undefined;
}

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface Placement {
  left: number;
  top: number;
  /** The panel's width, narrowed to the window where it would not fit. */
  width: number;
  side: "below" | "above" | "right" | "left";
}

/** Space kept between the panel and what it explains, and between the panel and the window's edge. */
const GAP = 8;

/**
 * Where the panel goes: below the element if it fits, else above, else beside it on whichever side
 * has room, else on the side with the most room. It is kept inside the window, and never overlaps the
 * element when any side has room for it, so a fader or a button stays in sight and in reach.
 */
export function placePanel(target: Rect, panel: { width: number; height: number }, viewport: { width: number; height: number }): Placement {
  const width = Math.min(panel.width, viewport.width - 2 * GAP);
  const height = panel.height;
  const clamp = (value: number, low: number, high: number) => Math.max(low, Math.min(value, Math.max(low, high)));
  const centredLeft = clamp(target.left + target.width / 2 - width / 2, GAP, viewport.width - GAP - width);
  const centredTop = clamp(target.top + target.height / 2 - height / 2, GAP, viewport.height - GAP - height);
  const room = {
    below: viewport.height - GAP - (target.top + target.height + GAP),
    above: target.top - GAP - GAP,
    right: viewport.width - GAP - (target.left + target.width + GAP),
    left: target.left - GAP - GAP,
  };
  if (room.below >= height) return { left: centredLeft, top: target.top + target.height + GAP, width, side: "below" };
  if (room.above >= height) return { left: centredLeft, top: target.top - GAP - height, width, side: "above" };
  if (room.right >= width) return { left: target.left + target.width + GAP, top: centredTop, width, side: "right" };
  if (room.left >= width) return { left: target.left - GAP - width, top: centredTop, width, side: "left" };
  // Nowhere it fits whole: the side with the most room, held inside the window.
  const best = (Object.entries(room) as [Placement["side"], number][]).sort((a, b) => b[1] - a[1])[0]?.[0] ?? "below";
  if (best === "below") return { left: centredLeft, top: clamp(target.top + target.height + GAP, GAP, viewport.height - GAP - height), width, side: best };
  if (best === "above") return { left: centredLeft, top: clamp(target.top - GAP - height, GAP, viewport.height - GAP - height), width, side: best };
  if (best === "right") return { left: clamp(target.left + target.width + GAP, GAP, viewport.width - GAP - width), top: centredTop, width, side: best };
  return { left: clamp(target.left - GAP - width, GAP, viewport.width - GAP - width), top: centredTop, width, side: best };
}
