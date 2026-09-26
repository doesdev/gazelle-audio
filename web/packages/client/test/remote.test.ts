// `pairPhone`: what a phone sends to pair and what it makes of the answer.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, pairPhone, type PairFetch } from "../src/index.ts";

test("pairing posts the code and name to the pair route and answers the token", async () => {
  const calls: { url: string; init: Parameters<PairFetch>[1] }[] = [];
  const fetch: PairFetch = async (url, init) => {
    calls.push({ url, init });
    return { ok: true, status: 200, json: async () => ({ token: "t0k3n", phone: { id: "p1", name: "Pixel", paired_ms: 1, last_seen_ms: 1, last_address: "192.168.1.50" } }) };
  };
  const paired = await pairPhone("http://192.168.1.5:8420", "ABCD-EFGH", "Pixel", fetch);
  assert.equal(paired.token, "t0k3n");
  assert.equal(paired.phone.name, "Pixel");
  assert.equal(calls[0]?.url, "http://192.168.1.5:8420/api/v1/remote/pair");
  assert.equal(calls[0]?.init.method, "POST");
  assert.deepEqual(JSON.parse(calls[0]?.init.body ?? ""), { code: "ABCD-EFGH", name: "Pixel" });
  assert.equal(calls[0]?.init.credentials, "same-origin", "so the browser keeps the cookie the answer sets");
});

test("a refusal comes back as the server's code", async () => {
  const fetch: PairFetch = async () => ({ ok: false, status: 403, json: async () => ({ error: { code: "pairing_refused", message: "That code is not valid now." } }) });
  await assert.rejects(pairPhone("http://h", "x", "y", fetch), (error: unknown) => error instanceof GazelleError && error.code === "pairing_refused");
  const unreachable: PairFetch = async () => {
    throw new Error("network down");
  };
  await assert.rejects(pairPhone("http://h", "x", "y", unreachable), (error: unknown) => error instanceof GazelleError && error.code === "not_connected");
});
