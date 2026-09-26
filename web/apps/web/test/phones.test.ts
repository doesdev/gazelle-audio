// The Workspace page's Phones section, below its DOM: what it reads and when, what it says about
// reaching Gazelle, the countdown, the list's times, and the QR code's drawing.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { ago, countdown, FIREWALL_NOTE, PAIRING_POLL_MS, PHONES_POLL_MS, PhonesModel, PLAIN_HTTP_NOTE, qrPath, reachText, type RemoteStatus } from "../src/store/phones.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, FakeClient, MemoryStorage } from "./fake-client.ts";

const settle = () => new Promise((resolve) => setImmediate(resolve));

function model(client = new FakeClient()) {
  const timers = new ManualTimers();
  const phones = new PhonesModel(client.remote, timers);
  return { client, timers, phones };
}

const ready = (phones: PhonesModel): RemoteStatus => {
  const state = phones.state.value;
  assert.equal(state.state, "ready");
  return (state as { status: RemoteStatus }).status;
};

test("following reads the status at once, then every ten seconds, and stops when nothing follows", async () => {
  const { client, timers, phones } = model();
  assert.equal(phones.state.value.state, "loading");
  const stop = phones.follow();
  await settle();
  assert.equal(ready(phones).allow_phones, false);
  assert.deepEqual(timers.pending(), [PHONES_POLL_MS]);
  timers.advance(PHONES_POLL_MS);
  await settle();
  assert.deepEqual(client.remoteCalls, ["status", "status"]);
  stop();
  stop();
  assert.deepEqual(timers.pending(), [], "nothing is read once the section has gone");
});

test("allowing phones, pairing and revoking go through the server and land in the state", async () => {
  const { client, timers, phones } = model();
  phones.follow();
  await settle();
  await phones.setAllowPhones(true);
  assert.equal(ready(phones).allow_phones, true);
  await phones.startPairing();
  assert.equal(ready(phones).pairing?.code, "ABCD-EFGH");
  assert.equal(timers.pending()[0], PAIRING_POLL_MS, "read more often while a phone may be pairing");

  // The phone pairs: the next read has no pairing and one more phone, which is named.
  client.remoteStatus = { ...ready(phones), pairing: null, phones: [{ id: "p1", name: "Pixel", paired_ms: 1, last_seen_ms: 1, last_address: "192.168.1.50" }] };
  timers.advance(PAIRING_POLL_MS);
  await settle();
  assert.equal(phones.justPaired.value, "Pixel");
  assert.equal(timers.pending()[0], PHONES_POLL_MS);

  await phones.revoke("p1");
  assert.deepEqual(ready(phones).phones, []);
  assert.deepEqual(client.remoteCalls.filter((c) => c !== "status"), ["allow:true", "pairing", "revoke:p1"]);
});

test("a refusal is said, and the next thing that works clears it", async () => {
  const { phones } = model();
  phones.follow();
  await settle();
  await phones.startPairing();
  assert.match(phones.problem.value ?? "", /Allow phones/);
  await phones.setAllowPhones(true);
  assert.equal(phones.problem.value, undefined);
});

test("on a phone the section is told it is elsewhere, not that something failed", async () => {
  const client = new FakeClient();
  client.remoteStatus = undefined;
  const { phones } = model(client);
  phones.follow();
  await settle();
  assert.deepEqual(phones.state.value, { state: "elsewhere" });
});

test("the store carries the section's model, talking to the client", async () => {
  const client = new FakeClient();
  const store = new Store(client, { storage: new MemoryStorage(), timers: new ManualTimers(), themeSources: builtInThemes });
  await store.phones.refresh();
  assert.equal(store.phones.state.value.state, "ready");
  assert.deepEqual(client.remoteCalls, ["status"]);
});

test("a phone's refusal of the updater stops the store asking, as a server without one does", async () => {
  const client = new FakeClient();
  const timers = new ManualTimers();
  client.failUpdates = new (await import("gazelle-audio-client")).GazelleError("not_local", "Only Gazelle on the computer itself can do that.");
  const store = new Store(client, { storage: new MemoryStorage(), timers, themeSources: builtInThemes });
  await store.start();
  await settle();
  assert.equal(store.update.value, undefined);
  assert.equal(timers.pending().length, 0, "no second read is scheduled");
});

const base: RemoteStatus = {
  allow_phones: true,
  fixed_by_bind: null,
  listening: true,
  error: null,
  port: 8420,
  addresses: [{ ip: "192.168.1.5", primary: true }],
  urls: ["http://192.168.1.5:8420/"],
  phones: [],
  pairing: null,
  now_ms: 0,
};

test("the reach line says where phones go, or exactly why they cannot", () => {
  assert.deepEqual(reachText({ ...base, allow_phones: false, listening: false }), { text: "Off: only this computer can reach Gazelle.", problem: false });
  assert.match(reachText(base).text, /http:\/\/192\.168\.1\.5:8420\//);
  assert.equal(reachText({ ...base, listening: false, error: "Gazelle could not listen on the network at port 8420: in use" }).problem, true);
  assert.equal(reachText({ ...base, urls: [], addresses: [] }).problem, true);
  assert.match(reachText({ ...base, fixed_by_bind: "0.0.0.0:8420" }).text, /--bind 0\.0\.0\.0:8420/);
});

test("the notes say what Windows may ask and what plain HTTP means", () => {
  assert.match(FIREWALL_NOTE, /private networks only/);
  assert.match(FIREWALL_NOTE, /never changes the firewall/);
  assert.match(PLAIN_HTTP_NOTE, /plain HTTP/);
  assert.match(PLAIN_HTTP_NOTE, /not who can watch/);
});

test("the countdown and the list's times read as a person would say them", () => {
  assert.equal(countdown(300_000, 0), "5:00");
  assert.equal(countdown(300_000, 53_500), "4:07");
  assert.equal(countdown(300_000, 400_000), "0:00");
  const minute = 60_000;
  assert.equal(ago(null, 0), "never");
  assert.equal(ago(0, 30_000), "just now");
  assert.equal(ago(0, minute), "a minute ago");
  assert.equal(ago(0, 5 * minute), "5 minutes ago");
  assert.equal(ago(0, 60 * minute), "an hour ago");
  assert.equal(ago(0, 24 * 60 * minute), "yesterday");
  assert.equal(ago(0, 3 * 24 * 60 * minute), "3 days ago");
});

test("the QR code is drawn as runs of dark modules inside a four module quiet zone", () => {
  const { d, size } = qrPath({ size: 3, rows: ["110", "001", "111"] });
  assert.equal(size, 11);
  assert.equal(d, "M4 4h2v1h-2zM6 5h1v1h-1zM4 6h3v1h-3z");
  assert.equal(qrPath({ size: 1, rows: ["0"] }).d, "");
});
