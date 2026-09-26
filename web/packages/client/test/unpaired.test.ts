// A phone the computer has revoked: the server closes its socket with CLOSE_UNPAIRED (4401), or
// answers a request 401 `unauthorized`. Either way the client stops for good rather than knocking
// again and again, and says so once.

import { test } from "node:test";
import assert from "node:assert/strict";

import { CLOSE_UNPAIRED, connect, GazelleError, type ConnectOptions, type FetchLike } from "../src/index.ts";
import { fakeNetwork, ManualTimers, outcome } from "./fakes.ts";

const HELLO = { type: "hello", version: "1.4.1", backend: "loopback", dry_run: false, devices: [], phone: true };

async function open(options: ConnectOptions = {}, hello: object = HELLO) {
  const net = fakeNetwork();
  const timers = new ManualTimers();
  const pending = connect("http://192.168.1.5:8420", { WebSocket: net.WebSocket, timers, random: () => 1, ...options });
  const socket = net.sockets[0]!;
  socket.receive(hello);
  const client = await pending;
  const told: string[] = [];
  const statuses: string[] = [];
  client.on("unpaired", (reason) => told.push(reason));
  client.on("status", (status) => statuses.push(status));
  return { client, net, timers, socket, told, statuses };
}

const flush = () => new Promise<void>((resolve) => setImmediate(resolve));

test("the hello says whether this connection is a phone's", async () => {
  assert.equal((await open()).client.server.phone, true);
  assert.equal((await open({}, { ...HELLO, phone: undefined })).client.server.phone, false, "a server too old to say is the computer");
});

test("a socket closed with 4401 stops the client: no reconnect, status closed, told once", async () => {
  const { client, net, socket, timers, told, statuses } = await open();
  socket.closed = true;
  socket.onclose?.({ code: CLOSE_UNPAIRED, reason: "this phone is no longer paired" });
  assert.equal(client.status, "closed");
  assert.equal(client.unpaired, true);
  assert.deepEqual(told, ["this phone is no longer paired"]);
  assert.deepEqual(statuses, ["closed"]);
  assert.deepEqual(timers.pending(), [], "no reconnect is scheduled");
  timers.advance(60_000);
  assert.equal(net.sockets.length, 1, "and none is attempted");
});

test("an ordinary close still reconnects", async () => {
  const { client, socket, timers, told } = await open();
  socket.closed = true;
  socket.onclose?.({ code: 1006, reason: "" });
  assert.equal(client.status, "reconnecting");
  assert.deepEqual(timers.pending(), [250]);
  assert.deepEqual(told, []);
  await client.close();
});

test("a request answered 401 unauthorized stops the client too", async () => {
  const fetch: FetchLike = async () => ({ ok: false, status: 401, json: async () => ({ error: { code: "unauthorized", message: "This device is not paired with Gazelle." } }) });
  const { client, socket, timers, told } = await open({ fetch });
  const answer = await outcome(client.workspace.get());
  assert.equal(!answer.ok && answer.error instanceof GazelleError ? answer.error.code : undefined, "unauthorized");
  assert.equal(client.status, "closed");
  assert.equal(socket.closed, true, "the socket is let go");
  assert.deepEqual(told, ["This device is not paired with Gazelle."]);
  assert.deepEqual(timers.pending(), []);
});

test("a phone that cannot get back in asks whether it is still paired, and stops when it is not", async () => {
  const asked: string[] = [];
  let paired = true;
  const fetch: FetchLike = async (url) => {
    asked.push(url);
    return paired ? { ok: true, status: 200, json: async () => ({ status: "ok" }) } : { ok: false, status: 401, json: async () => ({ error: { code: "unauthorized", message: "not paired" } }) };
  };
  const { client, net, socket, timers, told } = await open({ fetch });
  socket.drop();
  // The first try fails (the server is away): it asks, hears it is still paired, and keeps trying.
  timers.advance(timers.pending()[0]!);
  net.sockets.at(-1)!.drop();
  await flush();
  assert.deepEqual(asked, ["http://192.168.1.5:8420/api/v1/health"]);
  assert.equal(client.status, "reconnecting");
  assert.equal(timers.pending().length, 1);
  // Revoked meanwhile: the next failed try hears 401, and the knocking stops.
  paired = false;
  timers.advance(timers.pending()[0]!);
  net.sockets.at(-1)!.drop();
  await flush();
  assert.equal(client.status, "closed");
  assert.deepEqual(told, ["not paired"]);
  assert.deepEqual(timers.pending(), []);
});

test("the computer itself never asks: a failed reconnect there is only a server away", async () => {
  const asked: string[] = [];
  const fetch: FetchLike = async (url) => {
    asked.push(url);
    return { ok: true, status: 200, json: async () => ({}) };
  };
  const { client, net, socket, timers } = await open({ fetch }, { ...HELLO, phone: false });
  socket.drop();
  timers.advance(timers.pending()[0]!);
  net.sockets.at(-1)!.drop();
  await flush();
  assert.deepEqual(asked, []);
  await client.close();
});
