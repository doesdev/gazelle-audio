// The parameter catalogue arrives on its own (store/effect-parameters.ts): it is about 116 kB of
// generated tables that only the Effects page needs, so it is not in the app's bundle and nothing
// knows an effect's parameters until the page fetches it. Its own test file, because the catalogue
// is a module-wide singleton and node runs each test file in its own process: the tests below see
// it before it has been fetched, which no other file can.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { catalogue } from "../src/store/effect-parameters.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, MemoryStorage } from "./fake-client.ts";

function setup() {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"));
  client.respond = async (call) => ({ device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 1, dry_run: false, response: null, response_error: null });
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (callback) => callback(), themeSources: builtInThemes });
  return { client, store };
}

test("before it is fetched nothing knows an effect's parameters, and no read is sent", async () => {
  const { client, store } = setup();
  const effects = store.effects("loopback-0");
  assert.equal(catalogue.value, undefined, "not in the app's bundle");
  assert.equal(effects.catalogueReady, false);
  assert.equal(effects.description(39), undefined, "the PowerGate is unknown until the catalogue is here");
  assert.equal(await effects.readParameters(39, 2), false);
  assert.deepEqual(client.invocations.filter((c) => c.command.startsWith("get_")), [], "and nothing was asked of the device");

  // Two devices ask at once, as two pages would: one fetch serves both.
  const studio = store.effects("loopback-1");
  const [first, second] = await Promise.all([effects.loadCatalogue(), studio.loadCatalogue()]);
  assert.equal(first, second, "one catalogue, fetched once however many ask for it");
  assert.notEqual(catalogue.value, undefined);
  assert.equal(effects.catalogueReady, true);
  assert.equal(effects.description(39)?.get, "get_powergate_conf", "and it is there once fetched");
  assert.equal(effects.description(1), undefined);
  assert.match(effects.unsupportedReason(1) ?? "", /instance/, "an effect left out says why, from the catalogue");
  assert.equal(await effects.readParameters(39, 2), true, "and it is read now");
});

test("each model reads its own half of the one catalogue", async () => {
  const { store } = setup();
  const quadro = store.effects("loopback-0");
  const studio = store.effects("loopback-1");
  await quadro.loadCatalogue();
  assert.equal(studio.description(39)?.get, "get_powergate_configs", "the Studio+ reads every instance of a type at once");
  assert.equal(quadro.description(39)?.get, "get_powergate_conf");
});
