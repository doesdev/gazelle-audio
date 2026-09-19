// What the header says about updating, and how the store keeps up with the updater: the prompt
// each state makes, the polling, and the three calls a click can make. No server is started here.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type UpdateState, type UpdateStatus } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { Store, UPDATE_BUSY_POLL_MS, UPDATE_POLL_MS } from "../src/store/store.ts";
import { updatePrompt } from "../src/store/update.ts";
import { device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

/** An en dash and an em dash, by code point, so this file carries neither. */
const DASHES = new RegExp(`[${String.fromCodePoint(0x2013, 0x2014)}]`, "u");

const status = (state: UpdateState, rest: Partial<UpdateStatus> = {}): UpdateStatus => ({
  version: "1.1.0",
  target: "x86_64-pc-windows-msvc",
  channel: "stable",
  check: true,
  auto_download: true,
  can_verify: true,
  last_check_ms: 1_789_700_000_000,
  state,
  ...rest,
});

/** `null` is a server with no updater at all, which answers 404 on every update call. */
function setup(state: UpdateState | null = { state: "up_to_date" }) {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  client.updateStatus = state === null ? undefined : status(state);
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage: new MemoryStorage(), requestFrame: () => {}, themeSources: [] });
  return { client, timers, store };
}

// ---------------------------------------------------------------------------------------------
// What each state says
// ---------------------------------------------------------------------------------------------

test("an app that is up to date, checking, or has never checked says nothing at all", () => {
  for (const state of [{ state: "unknown" }, { state: "checking" }, { state: "up_to_date" }] as UpdateState[]) {
    assert.equal(updatePrompt(status(state)), undefined, `${state.state} is not worth a line`);
  }
  // No updater at all (a server bound off loopback, or --no-update) is the same silence.
  assert.equal(updatePrompt(undefined), undefined);
});

test("a version ready to restart into is the one thing that asks to be pressed twice", () => {
  const prompt = updatePrompt(status({ state: "staged", version: "1.2.0" }));
  assert.equal(prompt?.kind, "ready");
  assert.equal(prompt?.label, "1.2.0 ready, restart");
  assert.equal(prompt?.act, "restart");
  assert.equal(prompt?.confirm, true);
  assert.match(String(prompt?.title), /devices are let go/);
});

test("a download in progress is said out loud, and is not a button", () => {
  const prompt = updatePrompt(status({ state: "downloading", version: "1.2.0" }));
  assert.equal(prompt?.kind, "downloading");
  assert.equal(prompt?.label, "1.2.0 downloading");
  assert.equal(prompt?.act, undefined, "there is nothing to do but wait");
  assert.equal(prompt?.confirm, false);
});

test("an install that does not download by itself is offered the download, on one press", () => {
  const prompt = updatePrompt(status({ state: "available", version: "1.2.0", page: "https://example.invalid/r" }, { auto_download: false }));
  assert.equal(prompt?.label, "Get 1.2.0");
  assert.equal(prompt?.act, "download");
  assert.equal(prompt?.confirm, false);
});

test("a failure is never silent, and offers another try", () => {
  const prompt = updatePrompt(status({ state: "failed", message: "no connection", detail: "https://api.github.com/x: status 503" }));
  assert.equal(prompt?.kind, "failed");
  assert.equal(prompt?.label, "Update failed, try again");
  assert.equal(prompt?.act, "check");
  assert.equal(prompt?.title, "No connection. https://api.github.com/x: status 503");
});

/** The one rule the whole app is held to, checked where the strings are actually built. */
test("no label carries an em dash or an en dash", () => {
  const states: UpdateState[] = [
    { state: "staged", version: "1.2.0" },
    { state: "downloading", version: "1.2.0" },
    { state: "available", version: "1.2.0", page: "p" },
    { state: "failed", message: "no connection", detail: "d" },
  ];
  for (const state of states) {
    const prompt = updatePrompt(status(state));
    assert.doesNotMatch(`${prompt?.label} ${prompt?.title}`, DASHES, state.state);
  }
});

// ---------------------------------------------------------------------------------------------
// Keeping up with the server
// ---------------------------------------------------------------------------------------------

test("the status is read on start and then every half minute, faster while something is happening", async () => {
  const { client, timers, store } = setup({ state: "up_to_date" });
  await store.start();
  await flush();
  assert.deepEqual(client.updateCalls, ["status"]);
  assert.equal(store.updatePrompt.value?.kind, undefined, "up to date is a header with nothing extra in it");
  assert.deepEqual(timers.pending(), [UPDATE_POLL_MS]);

  // A release turns up and is fetched without anyone clicking: the header says so.
  client.updateStatus = status({ state: "downloading", version: "1.2.0" });
  timers.advance(UPDATE_POLL_MS);
  await flush();
  assert.equal(store.updatePrompt.value?.label, "1.2.0 downloading");
  assert.deepEqual(timers.pending(), [UPDATE_BUSY_POLL_MS], "a download changes again soon; ask sooner");

  client.updateStatus = status({ state: "staged", version: "1.2.0" });
  timers.advance(UPDATE_BUSY_POLL_MS);
  await flush();
  assert.equal(store.updatePrompt.value?.act, "restart");
  assert.deepEqual(timers.pending(), [UPDATE_POLL_MS], "nothing is moving now");
  await store.close();
});

test("a server that does not serve the update route is asked once and never again", async () => {
  const { client, timers, store } = setup(null);
  await store.start();
  await flush();
  assert.deepEqual(client.updateCalls, ["status"]);
  assert.equal(store.update.value, undefined);
  assert.deepEqual(timers.pending(), [], "nothing to say about updates here, and nothing to ask");
  await store.close();
});

test("a connection that is down for a moment is asked again", async () => {
  const { client, timers, store } = setup({ state: "up_to_date" });
  client.failUpdates = new GazelleError("not_connected", "GET failed");
  await store.start();
  await flush();
  assert.equal(store.update.value, undefined);
  assert.deepEqual(timers.pending(), [UPDATE_POLL_MS], "a hiccup is not a reason to stop asking");

  client.failUpdates = undefined;
  client.updateStatus = status({ state: "staged", version: "1.2.0" });
  timers.advance(UPDATE_POLL_MS);
  await flush();
  assert.equal(store.updatePrompt.value?.kind, "ready");
  await store.close();
});

test("closing the store stops the polling", async () => {
  const { client, timers, store } = setup({ state: "up_to_date" });
  await store.start();
  await flush();
  await store.close();
  assert.deepEqual(timers.pending(), []);
  timers.advance(UPDATE_POLL_MS * 10);
  await flush();
  assert.deepEqual(client.updateCalls, ["status"]);
});

// ---------------------------------------------------------------------------------------------
// What a press does
// ---------------------------------------------------------------------------------------------

test("a restart is asked for, and says so, since the connection is about to drop", async () => {
  const { client, store } = setup({ state: "staged", version: "1.2.0" });
  await store.start();
  await flush();

  await store.restartForUpdate();
  assert.deepEqual(client.updateCalls.slice(-1), ["restart"]);
  assert.deepEqual(
    store.notices.value.map((n) => [n.level, n.message]),
    [["info", "Restarting into Gazelle 1.2.0. This page reconnects when the server is back."]],
  );
  await store.close();
});

test("a restart the server refuses is reported and nothing is claimed", async () => {
  const { client, store } = setup({ state: "up_to_date" });
  await store.start();
  await flush();

  await store.restartForUpdate();
  assert.equal(store.notices.value[0]?.level, "error");
  assert.match(String(store.notices.value[0]?.message), /nothing to restart into/);
  assert.deepEqual(client.updateCalls.slice(-1), ["restart"]);
  await store.close();
});

test("checking and downloading update what the header shows, and a failure raises a notice", async () => {
  const { client, store } = setup({ state: "failed", message: "no connection", detail: "d" });
  await store.start();
  await flush();
  assert.equal(store.updatePrompt.value?.act, "check");

  client.updateStatus = status({ state: "downloading", version: "1.2.0" });
  await store.checkForUpdate();
  await flush();
  assert.equal(store.updatePrompt.value?.label, "1.2.0 downloading");

  client.updateStatus = status({ state: "staged", version: "1.2.0" });
  await store.downloadUpdate();
  await flush();
  assert.equal(store.updatePrompt.value?.act, "restart");

  client.failUpdates = new GazelleError("not_connected", "POST failed");
  await store.checkForUpdate();
  assert.match(String(store.notices.value.at(-1)?.message), /Could not check for an update/);
  await store.downloadUpdate();
  assert.match(String(store.notices.value.at(-1)?.message), /Could not download the update/);
  await store.close();
});
