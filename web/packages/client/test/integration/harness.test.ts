import { test } from "node:test";
import assert from "node:assert/strict";
import { connect as tcpConnect } from "node:net";

import { childEnv, NO_HARDWARE, startServer } from "./server.ts";

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
  const server = await startServer(["--backend", "loopback"]);
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

// The two halves of keeping this suite off the user's own interfaces, now that --backend defaults
// to usb (decision 0018). The other half — a server under GAZELLE_NO_HARDWARE refusing the usb
// backend — is asserted on the Rust side (crates/gazelle-audio-server/tests/cli.rs), where it can
// be exercised without a suite that would open a device if it regressed.

// The message has to be the harness's own. A server spawned without the flag is refused by
// GAZELLE_NO_HARDWARE too, and its refusal also says "--backend" — so a looser assertion passes
// whether or not the harness checks anything, which is exactly the regression worth catching.
const NOT_NAMED = /startServer needs an explicit backend/;

test("the harness refuses to start a server whose backend was not named, before spawning one", async () => {
  await assert.rejects(() => startServer(["--dry-run"]), NOT_NAMED, "a forgotten backend is an error, not a usb server");
  await assert.rejects(() => startServer([]), NOT_NAMED);
  await assert.rejects(() => startServer([], { webUi: true }), NOT_NAMED);
});

test("every server the harness starts runs under GAZELLE_NO_HARDWARE", () => {
  assert.equal(childEnv()[NO_HARDWARE], "1");
  assert.equal(childEnv()["PATH"], process.env["PATH"], "on top of this process's environment, not instead of it");
});
