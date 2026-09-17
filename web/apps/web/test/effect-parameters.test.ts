// The effect parameter editor's model (EffectsModel's parameters; specs/2026-09-17-effects-and-reverb.md,
// "Effect parameters"): each effect type's own get and set, from the generated catalogue.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { EFFECT_PARAMETERS, UNSUPPORTED_EFFECTS } from "../src/store/effect-parameters.ts";
import { bandKey, formatParameter, shownParameters } from "../src/store/effects.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage, type Invocation } from "./fake-client.ts";

type Replies = Record<string, (call: Invocation) => Record<string, unknown> | null>;

function setup(replies: Replies = {}, dryRun = false) {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"));
  client.respond = async (call) => {
    const reply = replies[call.command];
    const response = dryRun || reply === undefined ? null : reply(call);
    if (!dryRun && call.command.startsWith("get_") && response === null) throw new Error(`${call.command} refused`);
    return { device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 1, dry_run: dryRun, response, response_error: null };
  };
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (callback) => callback(), themeSources: builtInThemes });
  const sent = (deviceId: string, command: string) => client.invocations.filter((c) => c.deviceId === deviceId && c.command === command);
  return { client, store, sent };
}

const slots = (...effects: [type: number, inst: number][]) => Array.from({ length: 8 }, (_, i) => ({ type: effects[i]?.[0] ?? 0, inst: effects[i]?.[1] ?? 0 }));

/** Quadro chains: AFX IN 1 and 2 linked, each with a PowerGate then a PowerFFC; AFX IN 3 a FET-A76 and a Brainiac; AFX IN 4 a ClearQ. */
const quadroChains = (): Replies => {
  const chains: Record<number, [number, number][]> = { 0: [[39, 2], [2, 0]], 1: [[39, 3], [2, 1]], 2: [[9, 0], [85, 1]], 3: [[1, 0]] };
  return {
    get_afx_strip_order: (call) => ({ entries: [{ slots: slots(...(chains[Number(call.options?.["ext3"])] ?? [])) }] }),
    get_afx_links: () => ({ entries: [1, 0, 0, 0, 0, 0, 0].map((linked) => ({ linked })) }),
    get_reverb_config: () => null,
    get_reverb_returns: () => null,
    get_reverb_sends: () => null,
  };
};

const gate = { enabled: 1, threshold: 60, range: 4, attack: 250, decay: 80, hold: 1200, gain: 244 };

test("the catalogue is generated for both models, and says why an effect is left out", () => {
  const quadroGate = EFFECT_PARAMETERS.quadro.get(39);
  assert.equal(quadroGate?.set, "set_powergate_conf");
  assert.equal(quadroGate?.get, "get_powergate_conf");
  assert.equal(quadroGate?.instanceParam, "id");
  assert.deepEqual(quadroGate?.parameters.map((p) => [p.name, p.min, p.max, p.default]), [
    ["threshold", 0, 120, 90],
    ["range", 0, 20, 0],
    ["attack", 1, 750, 100],
    ["decay", 1, 1000, 50],
    ["hold", 0, 5000, 0],
    ["gain", -24, 12, 0],
  ]);
  assert.equal(EFFECT_PARAMETERS.studio.get(39)?.get, "get_powergate_configs");
  assert.equal(EFFECT_PARAMETERS.studio.get(39)?.replyCount, 16);
  assert.match(UNSUPPORTED_EFFECTS.quadro.get(1) ?? "", /instance/);
  assert.equal(EFFECT_PARAMETERS.quadro.has(1), false);
});

test("opening an effect reads it once: the Quadro names the instance, and enabled gives its bypass", async () => {
  const { store, sent } = setup({ ...quadroChains(), get_powergate_conf: () => ({ entries: [{ ...gate, enabled: 0 }] }) });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  assert.equal(effects.parameters(39, 2).value, undefined, "unknown before a read");

  assert.equal(await effects.readParameters(39, 2), true);
  assert.deepEqual(sent("loopback-0", "get_powergate_conf").map((c) => c.args), [{ id: 2 }]);
  assert.deepEqual(effects.parameters(39, 2).value, { known: true, values: { threshold: 60, range: 4, attack: 250, decay: 80, hold: 1200, gain: -12 } }, "a negative gain in an unsigned field reads back signed");
  assert.equal(effects.bypass(39, 2).value, true, "enabled 0: bypassed");
  assert.equal(await effects.readParameters(39, 2), false, "read once");
  assert.equal(sent("loopback-0", "get_powergate_conf").length, 1);

  effects.forget();
  assert.equal(await effects.readParameters(39, 2), true, "forgetting reads again");
  assert.equal(effects.readParameters(1, 0) instanceof Promise, true);
  assert.equal(await effects.readParameters(1, 0), false, "an unsupported effect is not read");
  assert.equal(sent("loopback-0", "get_eq_configs").length, 0);
});

test("the Studio+ reads every instance of a type at once, entry k being instance k", async () => {
  const { store, sent } = setup({
    get_afx_order: () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ slots: i === 0 ? slots([39, 7]) : slots() })) }),
    get_afx_links: () => ({ entries: Array.from({ length: 8 }, () => ({ linked: 0 })) }),
    get_reverb_config: () => null,
    get_powergate_configs: () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ ...gate, threshold: i, gain: -3, enabled: i === 7 ? 1 : 0, linked: 0 })) }),
  });
  const effects = store.effects("loopback-1");
  await effects.readOnce();
  assert.equal(await effects.readParameters(39, 7), true);
  assert.equal(await effects.readParameters(39, 9), false, "the one read covered every instance");
  assert.deepEqual(sent("loopback-1", "get_powergate_configs").map((c) => c.args), [undefined]);
  assert.equal(effects.parameters(39, 7).value?.values["threshold"], 7);
  assert.equal(effects.parameters(39, 9).value?.values["threshold"], 9);
  assert.equal(effects.parameters(39, 9).value?.values["gain"], -3, "the Studio+ declares gain signed");
  assert.deepEqual([effects.bypass(39, 7).value, effects.bypass(39, 9).value], [false, true]);
});

test("a change resends every parameter with type and instance, clamped, coalesced per instance; the linked partner follows", async () => {
  const { store, sent } = setup({
    ...quadroChains(),
    get_powergate_conf: (call) => ({ entries: [{ ...gate, threshold: 60 + Number(call.args?.["id"]) }] }),
  });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  assert.equal(effects.setParameter(0, 0, "threshold", 70), false, "refused before a read: a default must not overwrite the device");
  await effects.readParameters(39, 2);

  assert.equal(effects.setParameter(0, 0, "threshold", 500.4), true);
  assert.equal(effects.setParameter(0, 0, "gain", -24.2), true);
  await flush();
  const writes = sent("loopback-0", "set_powergate_conf");
  assert.deepEqual(writes.map((c) => c.args), [
    { type_id: 39, inst_id: 2, threshold: 120, range: 4, attack: 250, decay: 80, hold: 1200, gain: 244 },
    { type_id: 39, inst_id: 3, threshold: 120, range: 4, attack: 250, decay: 80, hold: 1200, gain: 244 },
    { type_id: 39, inst_id: 2, threshold: 120, range: 4, attack: 250, decay: 80, hold: 1200, gain: 232 },
    { type_id: 39, inst_id: 3, threshold: 120, range: 4, attack: 250, decay: 80, hold: 1200, gain: 232 },
  ], "the gain of -24 goes as its byte; the linked chain's same effect gets the same settings on its own instance");
  assert.deepEqual(writes.map((c) => c.options?.["coalesce"]), ["afx_params:39:2:loopback-0", "afx_params:39:3:loopback-0", "afx_params:39:2:loopback-0", "afx_params:39:3:loopback-0"]);
  assert.deepEqual(effects.parameters(39, 2).value?.values, { threshold: 120, range: 4, attack: 250, decay: 80, hold: 1200, gain: -24 });
  assert.equal(effects.parameters(39, 3).value?.values["threshold"], 120);

  assert.equal(effects.resetParameter(0, 0, "threshold"), true);
  await flush();
  assert.equal(sent("loopback-0", "set_powergate_conf").at(-2)?.args?.["threshold"], 90, "reset goes to the panel's starting value");
  assert.throws(() => effects.setParameter(0, 0, "nonsense", 1), RangeError);
  assert.throws(() => effects.setParameter(3, 0, "gain", 1), /not supported/, "ClearQ is left out");
});

test("menus take only their values, bit masks only their bits, hidden fields go back as read, and a failed send puts back what was known", async () => {
  const { store, sent, client } = setup({
    ...quadroChains(),
    get_compressor_configs: () => ({ entries: [{ enabled: 1, attack: 10000, release: 10000, taw: 65535, ratio: 100, gain: 0, ctrl: 0, threshold: 24, knee: 0, linked: 1 }] }),
    get_uad_1176_conf: () => ({ entries: [{ enabled: 1, input: 35, output: 46, attack: 78, release: 21, ratio: 1 }] }),
    get_Brainiac_conf: () => ({ entries: [{ enabled: 1, release: 0, attack: 0, range: 0, ratio: 8, thresh: 0, linlog: 0, mode: 0, sideSource: 7, sideChanN: 3 }] }),
  });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  await Promise.all([effects.readParameters(2, 0), effects.readParameters(2, 1), effects.readParameters(9, 0), effects.readParameters(85, 1)]);

  assert.equal(effects.setParameter(0, 1, "taw", 3000), false, "not one of the menu's values");
  assert.equal(effects.setParameter(0, 1, "taw", 5000), true);
  assert.throws(() => effects.setParameter(0, 1, "linked", 0), /not a control/);
  assert.equal(effects.setParameter(2, 0, "ratio", 0b10110), true);
  assert.equal(effects.setParameter(2, 1, "mode", 2), true);
  await flush();
  assert.deepEqual(sent("loopback-0", "set_compressor_cfg")[0]?.args, { type_id: 2, inst_id: 0, attack: 10000, release: 10000, taw: 5000, ratio: 100, gain: 0, ctrl: 0, threshold: 24, knee: 0, linked: 1 });
  assert.equal(sent("loopback-0", "set_uad_1176_conf")[0]?.args?.["ratio"], 0b0110, "only the four ratio bits");
  assert.deepEqual(sent("loopback-0", "set_Brainiac_conf")[0]?.args, { type_id: 85, inst_id: 1, release: 0, attack: 0, range: 0, ratio: 8, thresh: 0, linlog: 0, mode: 2, sideSource: 7, sideChanN: 3 }, "the sidechain source goes back as read");

  client.respond = async () => {
    throw new Error("gone");
  };
  effects.setParameter(2, 1, "ratio", 50);
  assert.equal(effects.parameters(85, 1).value?.values["ratio"], 50, "shown at once");
  await flush();
  assert.equal(effects.parameters(85, 1).value?.values["ratio"], 8, "and put back when it was not sent");
});

test("in a dry run an effect counts as read with the panel's starting values", async () => {
  const { store, sent } = setup(quadroChains(), true);
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  // A dry run reads no chains, so the page has no slots: the model still reads by type and instance.
  assert.equal(await effects.readParameters(85, 1), true);
  assert.deepEqual(effects.parameters(85, 1).value, { known: false, values: { release: 0, attack: 0, range: 0, ratio: 8, thresh: 0, linlog: 0, mode: 0, sideSource: 0, sideChanN: 0 } });
  assert.equal(effects.bypass(85, 1).value, undefined, "nothing was read, so bypass stays unknown");
  assert.equal(await effects.readParameters(85, 1), false);
  assert.equal(sent("loopback-0", "get_Brainiac_conf").length, 1);
});

const amp = { enabled: 1, model: 0, gain: 70, bass: 62, mid: 63, midfreq: 9, treble: 68, density: 4, presence: 50, volume: 89, boost: 12, mode1: 1, mode2: 1, mode3: 0, mode4: 1, mode5: 0, level: -6 };

test("the Guitar Amp's controls come from each model's own layout in the panel's code", () => {
  const quadro = EFFECT_PARAMETERS.quadro.get(3);
  const studio = EFFECT_PARAMETERS.studio.get(3);
  assert.equal(quadro?.status, "full");
  assert.equal(quadro?.layouts?.by, "model");
  const layout = (family: "quadro" | "studio", model: number) => EFFECT_PARAMETERS[family].get(3)?.layouts?.models.get(model);
  assert.deepEqual(layout("quadro", 0)?.map((c) => c.name), ["bass", "mid", "treble", "volume", "mode1"], "Darkface 65: four knobs and a bright switch");
  assert.deepEqual(layout("quadro", 0)?.find((c) => c.name === "mode1"), { name: "mode1", label: "Bright (mode 1)", control: "switch", min: 0, max: 1 });
  assert.deepEqual(layout("quadro", 2)?.find((c) => c.name === "mode1"), { name: "mode1", label: "Mode 1", control: "menu", min: 0, max: 2, options: [[0, "Raw"], [1, "Vintage"], [2, "Modern"]] }, "Modern CH3's three-way switch, named as its class names its positions");
  assert.deepEqual(layout("quadro", 6)?.filter((c) => c.control !== undefined).map((c) => c.label), ["Shift (mode 2)", "Shift (mode 3)", "Mode 4", "Mode 5"], "Marcus II's two shift buttons and two switches");
  assert.deepEqual(layout("quadro", 10)?.find((c) => c.name === "mode3")?.options, [[0, "Position 1"], [1, "Position 2"], [2, "Position 3"]], "an inherited three-way switch has no names of its own");
  assert.deepEqual(layout("quadro", 7)?.map((c) => c.name), ["mid", "volume", "mode2"]);
  assert.equal(layout("studio", 10), undefined, "the Studio+ build does not offer the Bass SuperTube VR");
  assert.deepEqual(layout("studio", 6), layout("quadro", 6), "both panels lay the models out alike");
  assert.deepEqual(studio?.layouts?.fields, quadro?.layouts?.fields);

  assert.ok(quadro);
  const shown = (values: Record<string, number>) => shownParameters(quadro, values).map((p) => p.name);
  assert.deepEqual(shown(amp), ["model", "bass", "mid", "treble", "volume", "mode1", "level"]);
  assert.deepEqual(shown({ ...amp, model: 2 }), ["model", "gain", "bass", "mid", "treble", "presence", "volume", "mode1", "level"]);
  assert.deepEqual(shown({ ...amp, model: 42 }), ["model", "level"], "a model the panel does not know shows only what every model has");
  assert.equal(shownParameters(quadro, amp).find((p) => p.name === "mode1")?.label, "Bright (mode 1)");
  assert.deepEqual(shownParameters(EFFECT_PARAMETERS.quadro.get(39)!, {}).map((p) => p.name), ["threshold", "range", "attack", "decay", "hold", "gain"], "an effect without layouts shows every control");
});

test("an amp's switches follow its model; values the model does not use go back as read", async () => {
  const replies = quadroChains();
  const order = replies["get_afx_strip_order"]!;
  const { store, sent } = setup({
    ...replies,
    get_afx_strip_order: (call) => (Number(call.options?.["ext3"]) === 4 ? { entries: [{ slots: slots([3, 1]) }] } : order(call)),
    get_guitar_amp_configs: () => ({ entries: [amp] }),
  });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  await effects.readParameters(3, 1);
  const all = (change: Record<string, number>) => ({ type_id: 3, inst_id: 1, ...Object.fromEntries(Object.entries(amp).filter(([name]) => name !== "enabled")), ...change });

  assert.throws(() => effects.setParameter(4, 0, "gain", 10), /Darkface 65 \(US\) does not use gain/, "Darkface has no gain knob");
  assert.throws(() => effects.setParameter(4, 0, "mode2", 0), /does not use mode2/);
  assert.equal(effects.setParameter(4, 0, "mode1", 0), true);
  assert.equal(effects.setParameter(4, 0, "mode1", 2), true, "a two-way switch takes 0 or 1");
  await flush();
  assert.deepEqual(sent("loopback-0", "set_guitar_amp_conf").map((c) => c.args), [all({ mode1: 0 }), all({ mode1: 1 })], "gain, midfreq, density, boost and the other switches as read");

  assert.equal(effects.setParameter(4, 0, "model", 2), true);
  await flush();
  assert.deepEqual(sent("loopback-0", "set_guitar_amp_conf").at(-1)?.args, all({ model: 2 }), "a model change sends only the model: the panel keeps every other setting");
  assert.equal(effects.setParameter(4, 0, "mode1", 3), false, "Modern CH3's switch has three positions");
  assert.equal(effects.setParameter(4, 0, "mode1", 2), true);
  assert.equal(effects.setParameter(4, 0, "gain", 64), true, "Modern CH3 has a gain knob");
  await flush();
  assert.deepEqual(sent("loopback-0", "set_guitar_amp_conf").at(-1)?.args, all({ model: 2, mode1: 2, gain: 64 }));
  assert.throws(() => effects.setParameter(4, 0, "mode2", 1), /does not use mode2/);
});

/** Studio+ Equalizer bands as a reply carries them, instance i's band 1 frequency telling them apart. */
const eqBands = (inst: number) => [
  { freq: 80, qual: 0, gain: 0, ftype: 4 },
  { freq: 200 + inst, qual: 70, gain: -300, ftype: 2 },
  { freq: 2000, qual: 120, gain: 250, ftype: 2 },
  { freq: 6000, qual: 50, gain: 0, ftype: 2 },
  { freq: 12000, qual: 0, gain: 400, ftype: 1 },
];
const studioEq = (part1: ((call: Invocation) => Record<string, unknown> | null) | undefined = undefined): Replies => ({
  get_afx_order: () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ slots: i === 0 ? slots([1, 9]) : i === 1 ? slots([1, 2]) : slots() })) }),
  get_afx_links: () => ({ entries: Array.from({ length: 8 }, () => ({ linked: 0 })) }),
  get_reverb_config: () => null,
  get_eq_configs: (call) => {
    const part = Number(call.options?.["ext3"]);
    if (part === 1 && part1 !== undefined) return part1(call);
    return { entries: Array.from({ length: 8 }, (_, k) => ({ biquads: eqBands(part * 8 + k), enabled: part * 8 + k === 9 ? 0 : 1 })) };
  },
});

test("the Studio+ Equalizer is catalogued band by band, read in two parts", () => {
  const eq = EFFECT_PARAMETERS.studio.get(1);
  assert.ok(eq);
  assert.equal(UNSUPPORTED_EFFECTS.studio.has(1), false);
  assert.match(UNSUPPORTED_EFFECTS.quadro.get(1) ?? "", /instance/, "the Quadro ClearQ still waits for a probe");
  assert.deepEqual([eq.set, eq.get, eq.replyCount, eq.readParts, eq.bands?.param, eq.bands?.list, eq.bands?.bands.length], ["set_eq_conf", "get_eq_configs", 8, 2, "strip_id", "biquads", 5]);
  const band = (b: number, name: string) => {
    const parameter = eq.bands?.bands[b]?.find((q) => q.name === name);
    assert.ok(parameter, `band ${b} ${name}`);
    return parameter;
  };
  assert.deepEqual(band(0, "ftype").options, [[0, "Low shelf"], [4, "High-pass"]]);
  assert.deepEqual(band(4, "ftype").options, [[1, "High shelf"], [3, "Low-pass"]]);
  assert.deepEqual([band(2, "ftype").hidden, band(2, "ftype").default], ["internal", 2], "a peak band's type is not a control");
  assert.deepEqual([band(0, "qual").hidden, band(1, "qual").min, band(1, "qual").max], ["internal", 50, 1800], "the outer bands have no Q");
  assert.deepEqual(band(0, "gain").offWhen, { name: "ftype", values: [4] });
  assert.deepEqual(eq.bands?.bands.map((b) => [b[0]?.min, b[0]?.max, b[0]?.default]), [[20, 800, 100], [20, 800, 100], [125, 8000, 2000], [400, 20000, 5000], [400, 20000, 5000]]);
  // As the panel's '{:.1f}'.format(value / 1000): an exact half rounds to even, anything else by the float's value.
  assert.deepEqual([11250, 11750, 11350, 14520, 1250].map((v) => formatParameter(band(3, "freq"), v)), ["11.2k", "11.8k", "11.3k", "14.5k", "1.2k"]);
  assert.equal(formatParameter(band(3, "freq"), 5000), "5k");
  assert.equal(formatParameter(band(3, "freq"), 800), "800");
  assert.equal(formatParameter(band(3, "gain"), -250), "-2.5");
  assert.equal(formatParameter(band(3, "qual"), 70), "0.70");
});

test("the Studio+ Equalizer reads both parts before any write, and one part failing leaves every instance unread", async () => {
  const { store, sent } = setup(studioEq());
  const effects = store.effects("loopback-1");
  await effects.readOnce();
  assert.equal(effects.setParameter(0, 0, "gain", 100, 2), false, "refused before a read");
  assert.equal(await effects.readParameters(1, 9), true);
  assert.deepEqual(sent("loopback-1", "get_eq_configs").map((c) => [c.args, c.options?.["ext3"]]), [[undefined, 0], [undefined, 1]], "part 0 and part 1, in ext3");
  assert.equal(await effects.readParameters(1, 2), false, "both parts cover every instance");
  const values = effects.parameters(1, 9).value;
  assert.equal(values?.known, true);
  assert.deepEqual([values?.values[bandKey("freq", 1)], values?.values[bandKey("gain", 1)], values?.values[bandKey("ftype", 0)]], [209, -300, 4], "instance 9 is part 1's entry 1");
  assert.equal(effects.parameters(1, 2).value?.values[bandKey("freq", 1)], 202);
  assert.deepEqual([effects.bypass(1, 9).value, effects.bypass(1, 2).value], [true, false]);

  const failing = setup(studioEq(() => null));
  const other = failing.store.effects("loopback-1");
  await other.readOnce();
  await other.readParameters(1, 2);
  assert.equal(other.parameters(1, 2).value, undefined, "part 0 answered, part 1 did not: nothing is taken");
  assert.equal(other.setParameter(1, 0, "gain", 100, 2), false);
  assert.equal(await other.readParameters(1, 2), true, "and it reads again");
  assert.equal(failing.sent("loopback-1", "get_eq_configs").length, 4);
});

test("an Equalizer change sends only its band, coalesced per band; a pass filter zeroes and holds the gain", async () => {
  const { store, sent } = setup(studioEq());
  const effects = store.effects("loopback-1");
  await effects.readOnce();
  await effects.readParameters(1, 9);

  assert.equal(effects.setParameter(0, 0, "gain", 250.4, 1), true);
  assert.equal(effects.setParameter(0, 0, "freq", 99999, 2), true);
  await flush();
  assert.deepEqual(sent("loopback-1", "set_eq_conf").map((c) => [c.args, c.options?.["coalesce"]]), [
    [{ type_id: 1, inst_id: 9, strip_id: 1, freq: 209, qual: 70, gain: 250, ftype: 2 }, "afx_params:1:9:1:loopback-1"],
    [{ type_id: 1, inst_id: 9, strip_id: 2, freq: 8000, qual: 120, gain: 250, ftype: 2 }, "afx_params:1:9:2:loopback-1"],
  ], "one band each, the frequency held to the band's range");

  assert.equal(effects.setParameter(0, 0, "gain", -600, 4), true);
  assert.equal(effects.setParameter(0, 0, "ftype", 3, 4), true);
  await flush();
  assert.deepEqual(sent("loopback-1", "set_eq_conf").at(-1)?.args, { type_id: 1, inst_id: 9, strip_id: 4, freq: 12000, qual: 0, gain: 0, ftype: 3 }, "a low-pass filter sends its gain as 0, as the panel does");
  assert.equal(effects.setParameter(0, 0, "gain", 300, 4), false, "and the gain stays 0 while it is a pass filter");
  assert.equal(effects.setParameter(0, 0, "ftype", 2, 4), false, "a high band is a shelf or a low-pass, not a peak");
  assert.equal(effects.setParameter(0, 0, "ftype", 1, 4), true);
  assert.equal(effects.setParameter(0, 0, "gain", 300, 4), true);
  await flush();
  assert.deepEqual(sent("loopback-1", "set_eq_conf").at(-1)?.args, { type_id: 1, inst_id: 9, strip_id: 4, freq: 12000, qual: 0, gain: 300, ftype: 1 });

  assert.throws(() => effects.setParameter(0, 0, "ftype", 2, 2), /not a control/, "a peak band's type");
  assert.throws(() => effects.setParameter(0, 0, "qual", 100, 0), /not a control/, "the low band's Q");
  assert.throws(() => effects.setParameter(0, 0, "gain", 100), /band/, "a band must be named");
  assert.throws(() => effects.setParameter(0, 0, "gain", 100, 5), /band/);
  assert.equal(effects.resetParameter(0, 0, "freq", 3), true);
  await flush();
  assert.deepEqual(sent("loopback-1", "set_eq_conf").at(-1)?.args, { type_id: 1, inst_id: 9, strip_id: 3, freq: 5000, qual: 50, gain: 0, ftype: 2 }, "reset to the band's starting frequency");
});

test("in a dry run the Studio+ Equalizer counts as read with the panel's starting bands", async () => {
  const { store, sent } = setup(studioEq(), true);
  const effects = store.effects("loopback-1");
  await effects.readOnce();
  assert.equal(await effects.readParameters(1, 15), true);
  assert.equal(sent("loopback-1", "get_eq_configs").length, 2);
  assert.deepEqual(effects.parameters(1, 15).value, {
    known: false,
    values: Object.fromEntries([100, 100, 2000, 5000, 5000].flatMap((freq, b) => [[bandKey("freq", b), freq], [bandKey("qual", b), b === 0 || b === 4 ? 0 : 50], [bandKey("gain", b), 0], [bandKey("ftype", b), [0, 2, 2, 2, 1][b]]])),
  });
});

test("parameters show as the panel's code shows them, and as the device's value where it gives no unit", () => {
  const find = (family: "quadro" | "studio", type: number, name: string) => {
    const parameter = EFFECT_PARAMETERS[family].get(type)?.parameters.find((p) => p.name === name);
    assert.ok(parameter, `${family} ${type} ${name}`);
    return parameter;
  };
  assert.equal(formatParameter(find("quadro", 39, "attack"), 255), "25.5");
  assert.equal(formatParameter(find("quadro", 2, "release"), 12500), "12.5");
  assert.equal(formatParameter(find("quadro", 2, "taw"), 10000), "RMS 100");
  assert.equal(formatParameter(find("quadro", 3, "level"), -6), "-6 dB");
  assert.equal(formatParameter(find("quadro", 3, "model"), 7), "Tweed Deluxe (US)");
  assert.equal(formatParameter(find("quadro", 9, "ratio"), 0b1001), "4:1 + 20:1");
  assert.equal(formatParameter(find("quadro", 9, "ratio"), 0), "None");
  assert.equal(formatParameter(find("quadro", 7, "phase_inv"), 1), "On");
  assert.equal(formatParameter(find("quadro", 7, "peak_freq"), 3), "3");
});
