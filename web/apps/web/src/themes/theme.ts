// Themes (decision P16): JSON profiles in the shape of VS Code colour themes. A theme has an
// optional `name`, `type` ("dark" | "light") and `extends` (another theme's id), a flat `colors`
// map of dotted keys, and a `meter` block whose gradient stops are placed in dBFS. Anything a
// theme leaves out comes from its `extends` chain and finally from gazelle-dark, which must be
// complete. Unknown keys are errors, because themes are hand-edited and typos should be loud.
// Ids: built-ins by file name (`gazelle-dark`), community themes `community:<file>`, user themes
// `user:<file>`.

export const BASE_THEME = "gazelle-dark";

export const COLOR_KEYS = [
  "surface.background",
  "surface.panel",
  "surface.raised",
  "surface.inset",
  "border.subtle",
  "border.strong",
  "section.header",
  "section.headerText",
  "text.primary",
  "text.secondary",
  "text.muted",
  "text.inverse",
  "accent",
  "accent.text",
  "focus",
  "control.background",
  "control.hover",
  "control.active",
  "control.text",
  "control.disabled",
  "control.disabledText",
  "fader.cap",
  "fader.capActive",
  "fader.track",
  "knob.ring",
  "state.mute",
  "state.solo",
  "state.read",
  "state.write",
  "state.dim",
  "state.talkback",
  "state.dryRun",
  "connection.open",
  "connection.reconnecting",
  "connection.closed",
  "notice.error",
  "notice.warning",
  "notice.info",
] as const;

export type ColorKey = (typeof COLOR_KEYS)[number];

/** Colours assigned to groups and channels in order; a list rather than one colour. */
export const PALETTE_KEY = "channel.palette";

export const METER_KEYS = ["peakHold", "clip", "background", "scale"] as const;
export type MeterKey = (typeof METER_KEYS)[number];

/** The dB span meter gradients are laid out over. */
export const METER_RANGE = { min: -60, max: 0 } as const;

export type ThemeType = "dark" | "light";
export type ThemeOrigin = "built-in" | "community" | "user";

export interface MeterStop {
  /** dBFS. */
  at: number;
  color: string;
}

export interface ResolvedTheme {
  id: string;
  origin: ThemeOrigin;
  name: string;
  type: ThemeType;
  colors: Record<ColorKey, string>;
  palette: string[];
  meter: Record<MeterKey, string> & { gradient: MeterStop[] };
}

export interface ThemeSource {
  id: string;
  origin: ThemeOrigin;
  data: unknown;
}

export interface ThemeProblem {
  id: string;
  origin: ThemeOrigin;
  errors: string[];
}

interface ThemeFile {
  name?: string;
  type?: ThemeType;
  extends?: string;
  colors?: Record<string, string | string[]>;
  meter?: Record<string, unknown>;
}

const TOP_LEVEL = ["$schema", "name", "type", "extends", "colors", "meter"];
const COLOR = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/;

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isColor(value: unknown): value is string {
  return typeof value === "string" && COLOR.test(value);
}

function gradientErrors(value: unknown): string[] {
  if (!Array.isArray(value) || value.length < 2) return ["meter.gradient must list at least two stops"];
  const errors: string[] = [];
  let previous = -Infinity;
  value.forEach((stop: unknown, i) => {
    if (!isObject(stop) || typeof stop["at"] !== "number" || !Number.isFinite(stop["at"]) || !isColor(stop["color"])) {
      errors.push(`meter.gradient[${i}] must be { "at": dBFS, "color": colour }`);
      return;
    }
    if (stop["at"] <= previous) errors.push(`meter.gradient[${i}].at must be greater than the stop before it`);
    previous = stop["at"];
  });
  return errors;
}

/** Everything wrong with a theme file, as readable messages; empty when it is usable. */
export function validateTheme(data: unknown): string[] {
  if (!isObject(data)) return ["a theme must be a JSON object"];
  const errors: string[] = [];
  for (const key of Object.keys(data)) if (!TOP_LEVEL.includes(key)) errors.push(`unknown top-level key "${key}"`);
  if (data["name"] !== undefined && typeof data["name"] !== "string") errors.push("name must be a string");
  if (data["type"] !== undefined && data["type"] !== "dark" && data["type"] !== "light") errors.push('type must be "dark" or "light"');
  if (data["extends"] !== undefined && typeof data["extends"] !== "string") errors.push("extends must be a theme id");
  const colors = data["colors"];
  if (colors !== undefined) {
    if (!isObject(colors)) {
      errors.push("colors must be an object");
    } else {
      for (const [key, value] of Object.entries(colors)) {
        if (key === PALETTE_KEY) {
          if (!Array.isArray(value) || value.length === 0 || !value.every(isColor)) errors.push(`colors["${key}"] must be a non-empty list of colours`);
        } else if (!(COLOR_KEYS as readonly string[]).includes(key)) {
          errors.push(`unknown colour "${key}"`);
        } else if (!isColor(value)) {
          errors.push(`colors["${key}"] must be #rgb, #rrggbb or #rrggbbaa, got ${JSON.stringify(value)}`);
        }
      }
    }
  }
  const meter = data["meter"];
  if (meter !== undefined) {
    if (!isObject(meter)) {
      errors.push("meter must be an object");
    } else {
      for (const [key, value] of Object.entries(meter)) {
        if (key === "gradient") errors.push(...gradientErrors(value));
        else if (!(METER_KEYS as readonly string[]).includes(key)) errors.push(`unknown meter key "${key}"`);
        else if (!isColor(value)) errors.push(`meter.${key} must be a colour, got ${JSON.stringify(value)}`);
      }
    }
  }
  return errors;
}

function completenessErrors(theme: ThemeFile): string[] {
  const errors = COLOR_KEYS.filter((key) => typeof theme.colors?.[key] !== "string").map((key) => `missing colour "${key}"`);
  if (!Array.isArray(theme.colors?.[PALETTE_KEY])) errors.push(`missing colour list "${PALETTE_KEY}"`);
  for (const key of [...METER_KEYS, "gradient"]) if (theme.meter?.[key] === undefined) errors.push(`missing meter.${key}`);
  return errors;
}

function merge(source: ThemeSource, layers: readonly ThemeFile[]): ResolvedTheme {
  const colors: Record<string, string> = {};
  const meter: Record<string, unknown> = {};
  let palette: string[] = [];
  let type: ThemeType = "dark";
  for (const layer of layers) {
    if (layer.type !== undefined) type = layer.type;
    for (const [key, value] of Object.entries(layer.colors ?? {})) {
      if (key === PALETTE_KEY) palette = [...(value as string[])];
      else colors[key] = value as string;
    }
    for (const [key, value] of Object.entries(layer.meter ?? {})) {
      meter[key] = key === "gradient" ? (value as MeterStop[]).map((stop) => ({ at: stop.at, color: stop.color })) : value;
    }
  }
  const own = source.data as ThemeFile;
  return {
    id: source.id,
    origin: source.origin,
    name: own.name ?? source.id,
    type,
    colors: colors as Record<ColorKey, string>,
    palette,
    meter: meter as ResolvedTheme["meter"],
  };
}

/**
 * Resolves every usable theme. Invalid themes, themes extending an unknown or invalid theme and
 * `extends` cycles are reported as problems instead. Throws if gazelle-dark is missing,
 * invalid or incomplete, since every other theme falls back to it.
 */
export function resolveThemes(sources: readonly ThemeSource[]): { themes: Map<string, ResolvedTheme>; problems: ThemeProblem[] } {
  const byId = new Map(sources.map((source) => [source.id, source]));
  const invalid = new Map<string, string[]>();
  for (const source of sources) {
    const errors = validateTheme(source.data);
    if (errors.length > 0) invalid.set(source.id, errors);
  }
  const base = byId.get(BASE_THEME);
  const baseErrors = base === undefined ? ["it is not among the sources"] : [...(invalid.get(BASE_THEME) ?? []), ...completenessErrors(base.data as ThemeFile)];
  if (baseErrors.length > 0) throw new Error(`the built-in ${BASE_THEME} theme is unusable: ${baseErrors.join("; ")}`);
  const baseFile = (base as ThemeSource).data as ThemeFile;

  const themes = new Map<string, ResolvedTheme>();
  const problems: ThemeProblem[] = [];
  for (const source of sources) {
    const errors = [...(invalid.get(source.id) ?? [])];
    const chain: ThemeFile[] = [];
    if (errors.length === 0) {
      const seen: string[] = [];
      let current: ThemeSource | undefined = source;
      while (current !== undefined) {
        if (seen.includes(current.id)) {
          errors.push(`extends forms a cycle: ${[...seen, current.id].join(" → ")}`);
          break;
        }
        seen.push(current.id);
        if (current !== source && invalid.has(current.id)) {
          errors.push(`extends the invalid theme "${current.id}"`);
          break;
        }
        const file = current.data as ThemeFile;
        chain.unshift(file);
        if (file.extends === undefined) break;
        current = byId.get(file.extends);
        if (current === undefined) errors.push(`extends the unknown theme "${file.extends}"`);
      }
    }
    if (errors.length > 0) problems.push({ id: source.id, origin: source.origin, errors });
    else themes.set(source.id, merge(source, [baseFile, ...chain]));
  }
  return { themes, problems };
}

/** `section.headerText` → `--ga-section-header-text`. */
export function cssVariable(key: string): string {
  return `--ga-${key.replace(/\./g, "-").replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`;
}

const round = (value: number) => Math.round(value * 100) / 100;

/**
 * A CSS gradient with each stop placed at its dB position on the meter's scale: linear over
 * `range` by default, or wherever `position` (dBFS → 0..100) puts it, for meters whose scale is
 * not linear.
 */
export function meterGradient(stops: readonly MeterStop[], range: { min: number; max: number } = METER_RANGE, direction = "to top", position?: (db: number) => number): string {
  const span = range.max - range.min;
  const place = position ?? ((db: number) => ((db - range.min) / span) * 100);
  const parts = stops.map((stop) => `${stop.color} ${round(Math.min(100, Math.max(0, place(stop.at))))}%`);
  return `linear-gradient(${direction}, ${parts.join(", ")})`;
}

/** The CSS custom properties that apply a theme. */
export function cssProperties(theme: ResolvedTheme): Record<string, string> {
  const properties: Record<string, string> = { "color-scheme": theme.type };
  for (const key of COLOR_KEYS) properties[cssVariable(key)] = theme.colors[key];
  theme.palette.forEach((color, i) => {
    properties[`--ga-channel-palette-${i}`] = color;
  });
  properties["--ga-channel-palette-count"] = String(theme.palette.length);
  for (const key of METER_KEYS) properties[cssVariable(`meter.${key}`)] = theme.meter[key];
  properties["--ga-meter-gradient"] = meterGradient(theme.meter.gradient);
  return properties;
}
