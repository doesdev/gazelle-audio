import { test } from "node:test";
import assert from "node:assert/strict";

import { connect, GazelleError, type Client, type ConnectOptions, type DeviceDescriptor, type FetchLike, type Workspace } from "../src/index.ts";
import { fakeNetwork, ManualTimers, outcome } from "./fakes.ts";

const device = (id: string, family: DeviceDescriptor["family"], pid: number): DeviceDescriptor => ({
  id,
  vid: 0x2982,
  pid,
  slug: family,
  model: family,
  family,
  command_count: family === null ? null : 1,
  identity_stable: true,
  backend: "loopback",
  max_packet_size: 64,
});
const QUADRO = device("loopback-0", "quadro", 0xa2f9);
const STUDIO = device("loopback-1", "studio", 0xa3f1);
const MYSTERY = device("usb:1:2:3:4", null, 0x0001);
const HELLO = { type: "hello", version: "0.1.0", backend: "loopback", dry_run: false, devices: [QUADRO, STUDIO, MYSTERY] };

async function open(options: ConnectOptions = {}) {
  const net = fakeNetwork();
  const timers = new ManualTimers();
  const pending = connect("http://127.0.0.1:8420", { WebSocket: net.WebSocket, timers, random: () => 1, ...options });
  const socket = net.sockets[0]!;
  socket.receive(HELLO);
  const client = await pending;
  return { client, net, timers, socket };
}

function quadro(client: Client) {
  const dev = client.device("loopback-0");
  if (dev.family !== "quadro") throw new Error(`expected a quadro, got ${dev.family}`);
  return dev;
}

/** The server's success reply to a sent frame. */
function reply(frame: Record<string, unknown>, response: unknown = null) {
  return {
    type: "rpc_response",
    id: frame["id"],
    result: { device_id: frame["device_id"], command: frame["command"], sent_hex: "00", sent_len: 1, dry_run: frame["dry_run"] === true, response, response_error: null },
  };
}

function valueOf<T>(settled: { ok: true; value: T } | { ok: false; error: unknown }): T {
  if (!settled.ok) throw settled.error;
  return settled.value;
}

function errorOf(settled: { ok: true; value: unknown } | { ok: false; error: unknown }): GazelleError {
  if (settled.ok) throw new Error("expected a rejection");
  if (!(settled.error instanceof GazelleError)) throw settled.error;
  return settled.error;
}

const flush = () => new Promise<void>((resolve) => setImmediate(resolve));
const volumes = (frames: Record<string, unknown>[]) => frames.map((f) => (f["args"] as { volume: number }).volume);

test("connect resolves on hello with the server's info and devices", async () => {
  const { client, socket } = await open();
  assert.equal(socket.url, "ws://127.0.0.1:8420/api/v1/ws");
  assert.equal(client.status, "open");
  assert.deepEqual(client.server, { version: "0.1.0", backend: "loopback", dry_run: false });
  assert.deepEqual([...client.devices.keys()], ["loopback-0", "loopback-1", "usb:1:2:3:4"]);
  await client.close();
});

test("connect fails without retrying when the socket closes before hello or hello never comes", async () => {
  const net = fakeNetwork();
  const timers = new ManualTimers();
  const closed = outcome(connect("http://127.0.0.1:1", { WebSocket: net.WebSocket, timers }));
  net.sockets[0]!.drop();
  assert.equal(errorOf(await closed).code, "closed");

  const silent = outcome(connect("http://127.0.0.1:1", { WebSocket: net.WebSocket, timers, timeoutMs: 1000 }));
  timers.advance(1000);
  assert.equal(errorOf(await silent).code, "timeout");
  assert.deepEqual(timers.pending(), [], "no reconnect is scheduled");
  assert.equal(net.sockets.length, 2);
});

test("invoke sends one frame and decodes the response's bytes", async () => {
  const { client, socket } = await open();
  const dev = quadro(client);

  const request = outcome(dev.invoke("get_assignment_request"));
  assert.deepEqual(socket.sent, [{ id: 1, device_id: "loopback-0", command: "get_assignment_request" }]);
  socket.receive(reply(socket.sent[0]!, { length: 2, request: "01ff" }));
  assert.deepEqual(valueOf(await request).response, { length: 2, request: new Uint8Array([1, 255]) });

  const trim = outcome(dev.invoke("set_trim_config", { trim_id: 1, control: 0, level: [new Uint8Array([1, 2]), "0304"] }, { dryRun: true }));
  assert.deepEqual(socket.sent[1], { id: 2, device_id: "loopback-0", command: "set_trim_config", args: { trim_id: 1, control: 0, level: ["0102", "0304"] }, dry_run: true });
  socket.receive(reply(socket.sent[1]!));
  const done = valueOf(await trim);
  assert.deepEqual([done.dry_run, done.response], [true, null]);
  await client.close();
});

test("an ext3 selector goes in the frame only when given", async () => {
  const { client, socket } = await open();
  const dev = quadro(client);
  const routing = outcome(dev.invoke("get_routing", undefined, { ext3: 11 }));
  assert.deepEqual(socket.sent[0], { id: 1, device_id: "loopback-0", command: "get_routing", ext3: 11 });
  socket.receive(reply(socket.sent[0]!));
  valueOf(await routing);

  const plain = outcome(dev.invoke("get_routing"));
  assert.equal("ext3" in socket.sent[1]!, false);
  socket.receive(reply(socket.sent[1]!));
  valueOf(await plain);
  await client.close();
});

test("server errors pass through with their code and detail", async () => {
  const { client, socket } = await open();
  const dev = quadro(client);
  const plain = outcome(dev.invoke("set_volume", { id: 1, volume: 300 }));
  socket.receive({ type: "rpc_error", id: 1, error: { code: "bad_value", message: "volume out of range" } });
  const error = errorOf(await plain);
  assert.deepEqual([error.code, error.message, error.detail], ["bad_value", "volume out of range", undefined]);

  const detailed = outcome(dev.invoke("set_volume", { id: 1, volume: 1 }));
  socket.receive({ type: "rpc_error", id: 2, error: { code: "device_gone", message: "gone", detail: { since: 3 } } });
  assert.deepEqual(errorOf(await detailed).detail, { since: 3 });
  await client.close();
});

test("a call without a reply times out after 5 s by default, or after its own timeout", async () => {
  const { client, socket, timers } = await open();
  const dev = quadro(client);
  let settled = false;
  const slow = outcome(dev.invoke("set_volume", { id: 1, volume: 1 })).then((o) => {
    settled = true;
    return o;
  });
  timers.advance(4999);
  await flush();
  assert.equal(settled, false);
  timers.advance(1);
  assert.equal(errorOf(await slow).code, "timeout");

  const quick = outcome(dev.invoke("set_volume", { id: 1, volume: 2 }, { timeoutMs: 100 }));
  timers.advance(100);
  assert.equal(errorOf(await quick).code, "timeout");
  socket.receive(reply(socket.sent[0]!)); // a late reply is ignored
  await client.close();
});

test("a lost connection fails waiting calls, refuses new ones and replays nothing", async () => {
  const { client, net, socket, timers } = await open();
  const statuses: string[] = [];
  client.on("status", (status) => statuses.push(status));
  const dev = quadro(client);

  const waiting = outcome(dev.invoke("set_volume", { id: 1, volume: 10 }));
  socket.drop();
  assert.equal(errorOf(await waiting).code, "closed");
  assert.equal(client.status, "reconnecting");
  assert.equal(errorOf(await outcome(dev.invoke("set_volume", { id: 1, volume: 11 }))).code, "not_connected");
  assert.equal(socket.sent.length, 1);

  assert.deepEqual(timers.pending(), [250]);
  timers.advance(250);
  const next = net.sockets[1]!;
  next.receive(HELLO);
  assert.equal(client.status, "open");
  assert.deepEqual(statuses, ["reconnecting", "open"]);
  assert.deepEqual(next.sent, [], "nothing is replayed");
  await client.close();
});

test("reconnect backs off from 250 ms, doubling to 5 s, with jitter", async () => {
  const { client, net, socket, timers } = await open();
  socket.drop();
  const delays: number[] = [];
  for (let i = 0; i < 7; i++) {
    const delay = timers.pending()[0]!;
    delays.push(delay);
    timers.advance(delay);
    net.sockets.at(-1)!.drop();
  }
  assert.deepEqual(delays, [250, 500, 1000, 2000, 4000, 5000, 5000]);

  timers.advance(timers.pending()[0]!);
  net.sockets.at(-1)!.receive(HELLO);
  net.sockets.at(-1)!.drop();
  assert.deepEqual(timers.pending(), [250], "a hello resets the backoff");
  await client.close();

  const low = await open({ random: () => 0 });
  low.socket.drop();
  assert.deepEqual(low.timers.pending(), [125], "jitter spans half to all of the ceiling");
  await low.client.close();
});

test("coalescing sends the call in flight and the latest waiting one, never a superseded one", async () => {
  const { client, socket } = await open();
  const dev = quadro(client);
  const fader = { coalesce: "monitor" };

  const first = outcome(dev.invoke("set_volume", { id: 0, volume: 10 }, fader));
  const second = outcome(dev.invoke("set_volume", { id: 0, volume: 20 }, fader));
  const third = outcome(dev.invoke("set_volume", { id: 0, volume: 30 }, fader));
  const other = outcome(dev.invoke("set_volume", { id: 1, volume: 99 }));
  assert.equal(errorOf(await second).code, "superseded");
  assert.deepEqual(volumes(socket.sent), [10, 99]);

  socket.receive(reply(socket.sent[0]!));
  valueOf(await first);
  assert.deepEqual(volumes(socket.sent), [10, 99, 30]);
  socket.receive(reply(socket.sent[2]!));
  valueOf(await third);
  socket.receive(reply(socket.sent[1]!));
  valueOf(await other);
  assert.ok(!volumes(socket.sent).includes(20), "the superseded value is never sent");

  // The key is free again, and a call waiting behind one in flight fails with the connection.
  const again = outcome(dev.invoke("set_volume", { id: 0, volume: 40 }, fader));
  const behind = outcome(dev.invoke("set_volume", { id: 0, volume: 50 }, fader));
  assert.deepEqual(volumes(socket.sent), [10, 99, 30, 40]);
  socket.drop();
  assert.deepEqual([errorOf(await again).code, errorOf(await behind).code], ["closed", "closed"]);
  assert.equal(socket.sent.length, 4);
  await client.close();
});

test("lagged and device events are delivered and keep devices current", async () => {
  const { client, socket } = await open();
  const seen: unknown[] = [];
  const offLagged = client.on("lagged", (missed) => seen.push(["lagged", missed]));
  client.on("device_added", (d) => seen.push(["added", d.id]));
  client.on("device_removed", (id) => seen.push(["removed", id]));

  socket.receive({ type: "lagged", missed: 12 });
  socket.receive({ type: "device_added", device: { ...QUADRO, id: "loopback-2" } });
  assert.equal(client.devices.get("loopback-2")?.family, "quadro");
  socket.receive({ type: "device_removed", device_id: "loopback-1" });
  assert.equal(client.devices.has("loopback-1"), false);
  offLagged();
  socket.receive({ type: "lagged", missed: 1 });

  assert.deepEqual(seen, [["lagged", 12], ["added", "loopback-2"], ["removed", "loopback-1"]]);
  assert.throws(() => client.device("loopback-1"), (e: unknown) => e instanceof GazelleError && e.code === "unknown_device");
  await client.close();
});

test("a reconnect's hello reports the devices that came and went", async () => {
  const { client, net, socket, timers } = await open();
  const seen: string[] = [];
  client.on("device_added", (d) => seen.push(`+${d.id}`));
  client.on("device_removed", (id) => seen.push(`-${id}`));
  const devices = client.devices;

  socket.drop();
  timers.advance(250);
  net.sockets[1]!.receive({ ...HELLO, devices: [QUADRO, { ...STUDIO, id: "loopback-9" }] });
  assert.deepEqual(seen.sort(), ["+loopback-9", "-loopback-1", "-usb:1:2:3:4"]);
  assert.equal(client.devices, devices, "the same map stays current");
  assert.deepEqual([...client.devices.keys()], ["loopback-0", "loopback-9"]);
  await client.close();
});

test("onCyclic delivers decoded fields for its own device and report", async () => {
  const { client, socket } = await open();
  const received: unknown[] = [];
  const off = quadro(client).onCyclic("0x73", (fields) => received.push(fields));
  const levels = [{ volume: 200, mute: 0, dim_on: 0, mono: 1, trim: 3 }];

  socket.receive({ type: "cyclic", device_id: "loopback-0", report_id: "0x73", fields: { current_preset: 2, pm_bank_src: "00010203", volumes: levels } });
  socket.receive({ type: "cyclic", device_id: "loopback-1", report_id: "0x73", fields: { current_preset: 5 } });
  socket.receive({ type: "cyclic", device_id: "loopback-0", report_id: "0x83", fields: { afx_meters: [] } });
  socket.receive({ type: "cyclic", device_id: "loopback-0", report_id: "0x073", fields: { current_preset: 3 } });
  off();
  socket.receive({ type: "cyclic", device_id: "loopback-0", report_id: "0x73", fields: { current_preset: 4 } });

  assert.deepEqual(received, [{ current_preset: 2, pm_bank_src: new Uint8Array([0, 1, 2, 3]), volumes: levels }, { current_preset: 3 }]);
  await client.close();
});

test("close fails waiting calls and stops reconnecting", async () => {
  const { client, socket, timers } = await open();
  const dev = quadro(client);
  const waiting = outcome(dev.invoke("set_volume", { id: 1, volume: 1 }));
  await client.close();
  assert.equal(errorOf(await waiting).code, "closed");
  assert.equal(client.status, "closed");
  assert.equal(socket.closed, true);
  socket.drop();
  assert.deepEqual(timers.pending(), []);
  assert.equal(errorOf(await outcome(dev.invoke("set_volume", { id: 1, volume: 1 }))).code, "not_connected");
});

test("the workspace goes over HTTP, and failures become GazelleError", async () => {
  const calls: { url: string; init: unknown }[] = [];
  const workspace: Workspace = { version: 1, groups: [], links: [], aliases: { "loopback-0": "Desk" }, mixers: {} };
  let next: { ok: boolean; status: number; body: unknown } | Error = { ok: true, status: 200, body: workspace };
  const fetch: FetchLike = async (url, init) => {
    calls.push({ url, init });
    const current = next;
    if (current instanceof Error) throw current;
    return { ok: current.ok, status: current.status, json: async () => current.body };
  };
  const { client } = await open({ fetch });

  assert.deepEqual(await client.workspace.get(), workspace);
  assert.deepEqual(await client.workspace.put(workspace), workspace);
  const url = "http://127.0.0.1:8420/api/v1/workspace";
  assert.deepEqual(calls, [
    { url, init: { method: "GET" } },
    { url, init: { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(workspace) } },
  ]);

  next = { ok: false, status: 500, body: { error: { code: "storage_error", message: "disk full" } } };
  const failed = errorOf(await outcome(client.workspace.put(workspace)));
  assert.deepEqual([failed.code, failed.message], ["storage_error", "disk full"]);
  next = new Error("ECONNREFUSED");
  assert.equal(errorOf(await outcome(client.workspace.get())).code, "not_connected");
  await client.close();
});

test("user themes are listed over HTTP", async () => {
  const listed = [{ file: "graphite.json", theme: { name: "Graphite", type: "dark" } }, { file: "broken.json", error: "expected value" }];
  const urls: string[] = [];
  const fetch: FetchLike = async (url) => {
    urls.push(url);
    return { ok: true, status: 200, json: async () => listed };
  };
  const { client } = await open({ fetch });
  assert.deepEqual(await client.themes(), listed);
  assert.deepEqual(urls, ["http://127.0.0.1:8420/api/v1/themes"]);
  await client.close();
});

test("a device of unknown model has no typed commands", async () => {
  const { client } = await open();
  const dev = client.device("usb:1:2:3:4");
  assert.equal(dev.family, null);
  assert.equal("invoke" in dev, false);
  await client.close();
});
