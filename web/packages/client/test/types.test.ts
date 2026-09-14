// Compile-time checks: `tsc` (the first step of `pnpm -C web test`) fails if a typed device
// accepts what it should not, because each `@ts-expect-error` below must really be an error.

import { test } from "node:test";
import assert from "node:assert/strict";

import type { Client } from "../src/index.ts";

export function typeChecks(client: Client): void {
  const dev = client.device("loopback-0");
  if (dev.family === "quadro") {
    void dev.invoke("set_volume", { id: 1, volume: 64 }, { coalesce: "monitor", dryRun: true });
    void dev.invoke("get_assignment_request").then((result) => {
      const bytes: Uint8Array | undefined = result.response?.request;
      return bytes;
    });
    dev.onCyclic("0x73", (fields) => {
      const level: number | undefined = fields.volumes[0]?.volume;
      const sources: Uint8Array = fields.pm_bank_src;
      return [level, sources];
    });
    // @ts-expect-error not a Quadro command
    void dev.invoke("set_line_gain", { id: 1, gain: 0 });
    // @ts-expect-error a required parameter is missing
    void dev.invoke("set_volume", { id: 1 });
    // @ts-expect-error parameters are numbers
    void dev.invoke("set_volume", { id: "1", volume: 64 });
    // @ts-expect-error parameters are required for this command
    void dev.invoke("set_volume");
    // @ts-expect-error not a Quadro report
    dev.onCyclic("0x99", () => undefined);
  }
  if (dev.family === "studio") {
    void dev.invoke("set_line_gain", { id: 1, gain: 0 });
  }
  if (dev.family === null) {
    // @ts-expect-error a device of unknown model has no commands
    void dev.invoke("set_volume", { id: 1, volume: 64 });
  }
}

test("typed device handles narrow by family (enforced by tsc)", () => {
  assert.equal(typeof typeChecks, "function");
});
