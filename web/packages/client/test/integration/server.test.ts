// The client against the real gazelle-audio-server (spec §9, §10 row 2): generated types agree
// with the server's own registry, invocation round-trips, cyclic events arrive decoded, and the
// workspace persists for the server's lifetime.

import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import { byteSize, type Field } from "../../../../tools/gen-types/src/field.ts";
import { connect, GazelleError, schemas, type Client, type FamilySchema, type FamilyTypes, type FieldDescriptor } from "../../src/index.ts";
import { startServer, type RunningServer } from "./server.ts";

let server: RunningServer;
let client: Client;

before(async () => {
  server = await startServer(["--loopback-cyclic-ms", "50"]);
  client = await connect(server.url);
});

after(async () => {
  await client?.close();
  await server?.stop();
});

interface ServerField {
  name: string;
  size: number;
}

interface ServerCommands {
  commands: { name: string; params: ServerField[]; returns: ServerField[] }[];
  cyclic_reports: { report_id: string; fields: ServerField[] }[];
}

const shape = (fields: readonly FieldDescriptor[]): ServerField[] => fields.map((f) => ({ name: f.name, size: byteSize(f as Field) }));

for (const [id, family] of [["loopback-0", "quadro"], ["loopback-1", "studio"]] as const) {
  test(`generated ${family} types agree with GET /devices/${id}/commands`, async () => {
    assert.equal(client.devices.get(id)?.family, family);
    const response = await fetch(`${server.url}/api/v1/devices/${id}/commands`);
    assert.equal(response.status, 200);
    const body = (await response.json()) as ServerCommands;
    const schema: FamilySchema = schemas[family];

    assert.deepEqual(body.commands.map((c) => c.name).sort(), Object.keys(schema.commands).sort());
    for (const command of body.commands) {
      const generated = schema.commands[command.name];
      assert.ok(generated, command.name);
      assert.deepEqual(command.params, shape(generated.params), `${family} ${command.name} params`);
      assert.deepEqual(command.returns, shape(generated.returns ?? []), `${family} ${command.name} returns`);
    }
    assert.deepEqual(body.cyclic_reports.map((r) => r.report_id).sort(), Object.keys(schema.cyclic).sort());
    for (const report of body.cyclic_reports) {
      assert.deepEqual(report.fields, shape(schema.cyclic[report.report_id] ?? []), `${family} cyclic ${report.report_id}`);
    }
  });
}

function quadro() {
  const dev = client.device("loopback-0");
  if (dev.family !== "quadro") throw new Error(`loopback-0 is ${dev.family}, not a quadro`);
  return dev;
}

test("invoke round-trips in dry run and live on the loopback", async () => {
  const dev = quadro();
  const dry = await dev.invoke("set_volume", { id: 1, volume: 64 }, { dryRun: true });
  assert.deepEqual([dry.device_id, dry.command, dry.dry_run, dry.response, dry.response_error], ["loopback-0", "set_volume", true, null, null]);
  assert.match(dry.sent_hex, /^([0-9a-f]{2})+$/);

  const live = await dev.invoke("set_volume", { id: 1, volume: 64 });
  assert.equal(live.dry_run, false);
  assert.equal(live.sent_hex, dry.sent_hex, "a live call sends the bytes the dry run reported");

  // The emulating loopback answers with the request's own contents, which are empty for a
  // command without parameters, so the server correlates the reply but cannot decode it. The
  // client passes the server's envelope through unchanged: no response, the decode error.
  const read = await dev.invoke("get_adats_links");
  assert.equal(read.dry_run, false);
  assert.equal(read.response, null);
  assert.match(read.response_error ?? "", /could not decode 0 bytes of response for 'get_adats_links'/);
});

test("server errors arrive as GazelleError with the server's code", async () => {
  const dev = quadro();
  await assert.rejects(dev.invoke("set_nothing_at_all" as "set_volume", { id: 1, volume: 1 }), (e: unknown) => e instanceof GazelleError && e.code === "unknown_command");
  await assert.rejects(dev.invoke("set_volume", { id: 1, volume: "zz" as unknown as number }), (e: unknown) => e instanceof GazelleError && e.code === "bad_value");
});

test("decoded cyclic events arrive with typed fields", async () => {
  const dev = quadro();
  const fields = await new Promise<FamilyTypes["quadro"]["cyclic"]["0x73"]>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("no 0x73 report within 5 s")), 5000);
    const off = dev.onCyclic("0x73", (report) => {
      clearTimeout(timer);
      off();
      resolve(report);
    });
  });
  assert.equal(typeof fields.current_preset, "number");
  assert.ok(fields.pm_bank_src instanceof Uint8Array, "byte arrays arrive as Uint8Array");
  assert.equal(fields.pm_bank_src.length, 4);
  assert.equal(fields.volumes.length, 6, "struct arrays arrive as arrays of objects");
  assert.equal(typeof fields.volumes[0]?.trim, "number");
});

test("the workspace round-trips over HTTP", async () => {
  const workspace = await client.workspace.get();
  assert.equal(workspace.version, 1);
  await client.workspace.put({ ...workspace, aliases: { ...workspace.aliases, "loopback-0": "Desk" } });
  assert.equal((await client.workspace.get()).aliases["loopback-0"], "Desk");
});

test("when the server goes away the client reports reconnecting and refuses calls", async () => {
  const own = await startServer();
  const watcher = await connect(own.url);
  try {
    const reconnecting = new Promise<void>((resolve) => {
      watcher.on("status", (status) => {
        if (status === "reconnecting") resolve();
      });
    });
    await own.stop();
    await reconnecting;
    const dev = watcher.device("loopback-0");
    if (dev.family !== "quadro") throw new Error("loopback-0 is not a quadro");
    await assert.rejects(dev.invoke("set_volume", { id: 1, volume: 1 }), (e: unknown) => e instanceof GazelleError && e.code === "not_connected");
  } finally {
    await watcher.close();
    await own.stop();
  }
});
