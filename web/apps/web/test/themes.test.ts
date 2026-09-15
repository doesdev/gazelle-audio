import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";

import { BASE_THEME, COLOR_KEYS, cssProperties, cssVariable, METER_KEYS, meterGradient, PALETTE_KEY, resolveThemes, validateTheme, type ThemeSource } from "../src/themes/theme.ts";

const json = (url: URL): unknown => JSON.parse(readFileSync(url, "utf8"));
const BUILT_IN = new URL("../themes/", import.meta.url);
const COMMUNITY = new URL("../../../themes/", import.meta.url);

function builtIns(): ThemeSource[] {
  return ["gazelle-dark", "gazelle-light"].map((id) => ({ id, origin: "built-in" as const, data: json(new URL(`${id}.json`, BUILT_IN)) }));
}

function community(): ThemeSource[] {
  return readdirSync(COMMUNITY)
    .filter((name) => name.endsWith(".json"))
    .map((name) => ({ id: `community:${name.slice(0, -5)}`, origin: "community" as const, data: json(new URL(name, COMMUNITY)) }));
}

test("the built-in and community themes are valid and resolve", () => {
  const sources = [...builtIns(), ...community()];
  for (const source of sources) assert.deepEqual(validateTheme(source.data), [], source.id);
  const { themes, problems } = resolveThemes(sources);
  assert.deepEqual(problems, []);
  assert.deepEqual([...themes.keys()], sources.map((s) => s.id));
  assert.ok(community().length > 0, "at least one community theme ships as an example");
});

test("gazelle-dark and gazelle-light define every key, and the schema describes exactly those keys", () => {
  const schema = json(new URL("theme.schema.json", BUILT_IN)) as {
    properties: { colors: { properties: Record<string, unknown> }; meter: { properties: Record<string, unknown> } };
  };
  const expectedColors = [...COLOR_KEYS, PALETTE_KEY].sort();
  assert.deepEqual(Object.keys(schema.properties.colors.properties).sort(), expectedColors);
  assert.deepEqual(Object.keys(schema.properties.meter.properties).sort(), [...METER_KEYS, "gradient"].sort());
  for (const { id, data } of builtIns()) {
    const theme = data as { colors: Record<string, unknown>; meter: Record<string, unknown> };
    assert.deepEqual(Object.keys(theme.colors).sort(), expectedColors, id);
    assert.deepEqual(Object.keys(theme.meter).sort(), [...METER_KEYS, "gradient"].sort(), id);
  }
});

test("validation names every mistake", () => {
  assert.deepEqual(validateTheme([1]), ["a theme must be a JSON object"]);
  assert.deepEqual(
    validateTheme({
      name: 3,
      type: "dusk",
      extends: false,
      colour: {},
      colors: { accent: "blue", "text.primray": "#fff", [PALETTE_KEY]: [] },
      meter: { gradient: [{ at: 0, color: "#fff" }, { at: -6, color: "#000" }], glow: "#fff", clip: "#ff0000" },
    }),
    [
      'unknown top-level key "colour"',
      "name must be a string",
      'type must be "dark" or "light"',
      "extends must be a theme id",
      'colors["accent"] must be #rgb, #rrggbb or #rrggbbaa, got "blue"',
      'unknown colour "text.primray"',
      `colors["${PALETTE_KEY}"] must be a non-empty list of colours`,
      "meter.gradient[1].at must be greater than the stop before it",
      'unknown meter key "glow"',
    ],
  );
  assert.deepEqual(validateTheme({ meter: { gradient: [{ at: 0, color: "#fff" }] } }), ["meter.gradient must list at least two stops"]);
});

test("extends chains layer themes over gazelle-dark, and the theme's own name wins", () => {
  const sources: ThemeSource[] = [
    ...builtIns(),
    { id: "user:warm", origin: "user", data: { extends: "gazelle-light", colors: { accent: "#e08a2e" } } },
    { id: "user:warmer", origin: "user", data: { name: "Warmer", extends: "user:warm", meter: { gradient: [{ at: -40, color: "#111111" }, { at: 0, color: "#eeeeee" }] } } },
  ];
  const { themes, problems } = resolveThemes(sources);
  assert.deepEqual(problems, []);
  const light = themes.get("gazelle-light");
  const warmer = themes.get("user:warmer");
  assert.ok(light && warmer);
  assert.equal(warmer.name, "Warmer");
  assert.equal(themes.get("user:warm")?.name, "user:warm", "an unnamed theme is listed by its id");
  assert.equal(warmer.type, "light", "type comes from the chain");
  assert.equal(warmer.colors.accent, "#e08a2e", "from user:warm");
  assert.equal(warmer.colors["surface.panel"], light.colors["surface.panel"], "from gazelle-light");
  assert.deepEqual(warmer.meter.gradient, [{ at: -40, color: "#111111" }, { at: 0, color: "#eeeeee" }]);
  assert.equal(warmer.meter.peakHold, light.meter.peakHold);
});

test("unknown parents, invalid parents and cycles are problems, not themes", () => {
  const { themes, problems } = resolveThemes([
    ...builtIns(),
    { id: "user:orphan", origin: "user", data: { extends: "user:nowhere" } },
    { id: "user:broken", origin: "user", data: { colors: { accent: "nope" } } },
    { id: "user:child", origin: "user", data: { extends: "user:broken" } },
    { id: "user:a", origin: "user", data: { extends: "user:b" } },
    { id: "user:b", origin: "user", data: { extends: "user:a" } },
  ]);
  assert.deepEqual([...themes.keys()], ["gazelle-dark", "gazelle-light"]);
  assert.deepEqual(problems, [
    { id: "user:orphan", origin: "user", errors: ['extends the unknown theme "user:nowhere"'] },
    { id: "user:broken", origin: "user", errors: ['colors["accent"] must be #rgb, #rrggbb or #rrggbbaa, got "nope"'] },
    { id: "user:child", origin: "user", errors: ['extends the invalid theme "user:broken"'] },
    { id: "user:a", origin: "user", errors: ["extends forms a cycle: user:a → user:b → user:a"] },
    { id: "user:b", origin: "user", errors: ["extends forms a cycle: user:b → user:a → user:b"] },
  ]);
});

test("gazelle-dark must be present and complete", () => {
  assert.throws(() => resolveThemes([]), /gazelle-dark theme is unusable: it is not among the sources/);
  const incomplete = { id: BASE_THEME, origin: "built-in" as const, data: { colors: { accent: "#ffffff" } } };
  assert.throws(() => resolveThemes([incomplete]), /missing colour "surface.background"/);
});

test("a theme becomes CSS custom properties with a dB-placed meter gradient", () => {
  const { themes } = resolveThemes(builtIns());
  const dark = themes.get(BASE_THEME);
  assert.ok(dark);
  const properties = cssProperties(dark);
  assert.equal(cssVariable("section.headerText"), "--ga-section-header-text");
  assert.equal(properties["--ga-surface-panel"], dark.colors["surface.panel"]);
  assert.equal(properties["--ga-meter-peak-hold"], dark.meter.peakHold);
  assert.equal(properties["--ga-channel-palette-0"], dark.palette[0]);
  assert.equal(properties["--ga-channel-palette-count"], String(dark.palette.length));
  assert.equal(properties["color-scheme"], "dark");
  assert.equal(Object.keys(properties).filter((k) => k.startsWith("--ga-") && !k.startsWith("--ga-channel-palette")).length, COLOR_KEYS.length + METER_KEYS.length + 1);
  assert.equal(
    meterGradient([{ at: -60, color: "#000" }, { at: -18, color: "#0f0" }, { at: -6, color: "#ff0" }, { at: 6, color: "#f00" }]),
    "linear-gradient(to top, #000 0%, #0f0 70%, #ff0 90%, #f00 100%)",
  );
  const halfway = (db: number) => (db < -20 ? 10 : 60);
  assert.equal(meterGradient([{ at: -40, color: "#000" }, { at: -6, color: "#fff" }], undefined, "to top", halfway), "linear-gradient(to top, #000 10%, #fff 60%)", "a non-linear scale places the stops");
});
