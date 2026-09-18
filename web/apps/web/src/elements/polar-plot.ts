// A small picture of a microphone's polar pattern (P72), for the Mic emulation rows of the Inputs
// page. The geometry is pure and tested; `polarPlot` draws it as inline SVG in the page's theme.
// Front is up. The part of a pattern with inverted polarity (a figure-8's rear lobe) is drawn
// dashed and unfilled, since it hears as loudly as the front but out of phase.

const SVG = "http://www.w3.org/2000/svg";

/** Samples round the circle: every 5°, so the front, sides and rear each fall on one. */
const SAMPLES = 72;

/** Rounds away floating-point error at a null, so a zero counts as the lobe it closes. */
const EPSILON = 1e-9;

/**
 * A first-order pattern's response at `theta` radians from the front, for a polar angle of +1
 * omni, 0 cardioid and -1 figure-8 (P68): `(1 + a) / 2 + ((1 - a) / 2) · cos θ`. Negative where
 * the capsule hears in inverted polarity.
 */
export function polarResponse(angle: number, theta: number): number {
  return (1 + angle) / 2 + ((1 - angle) / 2) * Math.cos(theta);
}

/** One lobe of a pattern, in unit coordinates with the centre at 0, 0 and y down as SVG has it. */
export interface Lobe {
  /** Inverted polarity: the rear lobe past cardioid. */
  negative: boolean;
  points: readonly (readonly [x: number, y: number])[];
}

/**
 * A pattern as lobes of |r|, turned `rotation` degrees clockwise. A pattern with nulls is split at
 * them, and each such lobe begins and ends at the centre, so every lobe closes on its own.
 */
export function polarLobes(angle: number, rotation = 0): Lobe[] {
  const turn = (rotation * Math.PI) / 180;
  const samples = Array.from({ length: SAMPLES }, (_, k) => {
    const theta = (k / SAMPLES) * 2 * Math.PI;
    const r = polarResponse(angle, theta);
    const point = [Math.abs(r) * Math.sin(theta + turn), -Math.abs(r) * Math.cos(theta + turn)] as const;
    return { negative: r < -EPSILON, point };
  });
  // Start at a change of polarity when there is one, so no lobe is cut in two by where the circle starts.
  const start = samples.findIndex((s, k) => s.negative !== (samples.at(k - 1) as (typeof samples)[number]).negative);
  if (start === -1) return [{ negative: samples[0]?.negative ?? false, points: samples.map((s) => s.point) }];
  const ordered = [...samples.slice(start), ...samples.slice(0, start)];
  const lobes: { negative: boolean; points: (readonly [number, number])[] }[] = [];
  for (const sample of ordered) {
    const current = lobes.at(-1);
    if (current !== undefined && current.negative === sample.negative) current.points.push(sample.point);
    else lobes.push({ negative: sample.negative, points: [[0, 0], sample.point] });
  }
  for (const lobe of lobes) lobe.points.push([0, 0]);
  return lobes;
}

/**
 * How a stereo technique wants two heads turned, in degrees clockwise, given each head's polar
 * angle. XY and Blumlein cross the heads at 90°; M/S turns the figure-8 side across the mid, its
 * positive lobe to the left. This is the technique's intent: the app cannot sense how the heads sit.
 */
export function stereoOrientation(technique: string, angles: readonly [bottom: number, top: number]): [bottom: number, top: number] {
  if (technique === "XY" || technique === "Blumlein") return [-45, 45];
  if (technique === "M/S") return angles[0] < angles[1] ? [-90, 0] : [0, -90];
  return [0, 0];
}

/** A lobe as an SVG path, scaled to `radius` about a centre at `centre`, `centre`. */
export function lobePath(points: readonly (readonly [number, number])[], radius: number, centre: number): string {
  const at = (v: number) => String(Math.round((centre + v * radius) * 100) / 100);
  return `${points.map(([x, y], k) => `${k === 0 ? "M" : "L"}${at(x)} ${at(y)}`).join("")}Z`;
}

export const POLAR_PLOT_STYLES = `
  .polar-plot { flex: none; display: block; overflow: visible; }
  .polar-plot .grid { fill: var(--ga-surface-inset); stroke: var(--ga-border-subtle); stroke-width: 1; }
  .polar-plot .axis { stroke: var(--ga-border-subtle); stroke-width: 1; }
  .polar-plot .lobe { fill: currentColor; fill-opacity: 0.25; stroke: currentColor; stroke-width: 1.25; stroke-linejoin: round; }
  .polar-plot .lobe.negative { fill: none; stroke-dasharray: 2 1.5; }
  .polar-plot .tone-0 { color: var(--ga-accent); }
  .polar-plot .tone-1 { color: var(--ga-text-primary); }
  .polar-plot[aria-disabled="true"] { opacity: 0.55; }
`;

function svg<K extends keyof SVGElementTagNameMap>(tag: K, attributes: Record<string, string | number>, ...children: SVGElement[]): SVGElementTagNameMap[K] {
  const element = document.createElementNS(SVG, tag);
  for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, String(value));
  element.append(...children);
  return element;
}

/** One head in a plot: its polar angle, how far it is drawn turned, and its tone (0 or 1). */
export interface PlotHead {
  angle: number;
  rotation?: number;
  tone?: number;
}

/**
 * A polar plot of one or more heads overlaid, `size` pixels square. It is an image named by
 * `label`, which also shows as its tooltip; the select beside it is what sets the pattern.
 */
export function polarPlot(heads: readonly PlotHead[], label: string, size = 32): SVGSVGElement {
  const centre = size / 2;
  const radius = centre - 1.5;
  const lobes = heads.flatMap((head) =>
    polarLobes(head.angle, head.rotation ?? 0).map((lobe) => svg("path", { class: `lobe tone-${head.tone ?? 0}${lobe.negative ? " negative" : ""}`, d: lobePath(lobe.points, radius, centre) })),
  );
  const title = svg("title", {});
  title.textContent = label;
  return svg(
    "svg",
    { class: "polar-plot", width: size, height: size, viewBox: `0 0 ${size} ${size}`, role: "img", "aria-label": label, "data-explain": "mic.plot" },
    title,
    svg("circle", { class: "grid", cx: centre, cy: centre, r: radius }),
    svg("line", { class: "axis", x1: centre, y1: centre - radius, x2: centre, y2: centre + radius }),
    svg("line", { class: "axis", x1: centre - radius, y1: centre, x2: centre + radius, y2: centre }),
    ...lobes,
  );
}
