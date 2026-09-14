import { test } from "node:test";
import assert from "node:assert/strict";
import { connect as tcpConnect } from "node:net";

import { startServer } from "./server.ts";

/** Resolves true when nothing accepts connections on the port. */
function refused(port: number): Promise<boolean> {
  return new Promise((done) => {
    const socket = tcpConnect({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      done(false);
    });
    socket.once("error", () => done(true));
  });
}

test("the harness starts the built server on a free port, stops it, and the port is released", async () => {
  const server = await startServer();
  try {
    const health = await fetch(`${server.url}/api/v1/health`);
    assert.equal(health.status, 200);
    assert.equal(((await health.json()) as { status: string }).status, "ok");
  } finally {
    await server.stop();
  }
  assert.notEqual(server.child.exitCode ?? server.child.signalCode, null, "the process has exited");
  assert.equal(await refused(server.port), true, "nothing listens on the port any more");
  await server.stop();
});
