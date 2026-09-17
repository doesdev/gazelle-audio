// The effect parameter editor's model (EffectsModel's parameters; specs/2026-09-17-effects-and-reverb.md,
// "Effect parameters"): each effect type's own get and set, from the generated catalogue.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { EFFECT_PARAMETERS, UNSUPPORTED_EFFECTS } from "../src/store/effect-parameters.ts";
import { formatParameter } from "../src/store/effects.ts";
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
