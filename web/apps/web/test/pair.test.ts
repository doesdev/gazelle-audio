// The pair page's logic: the code from the QR code's address, a name for the phone, and what it
// says for each way pairing can go.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError } from "gazelle-audio-client";

import { codeFromFragment, defaultPhoneName, pairFailureText, submitPair } from "../src/store/pair.ts";

test("the code comes from the address's fragment, and nothing else there counts", () => {
  assert.equal(codeFromFragment("#code=ABCD-EFGH"), "ABCD-EFGH");
  assert.equal(codeFromFragment("code=abcd-efgh"), "abcd-efgh");
  assert.equal(codeFromFragment("#code=%20ABCD-EFGH%20"), "ABCD-EFGH");
  assert.equal(codeFromFragment("#other=1&code=WXYZ-2345"), "WXYZ-2345");
  assert.equal(codeFromFragment(""), undefined);
  assert.equal(codeFromFragment("#code="), undefined);
  assert.equal(codeFromFragment("#/mixer"), undefined);
});

test("a phone is named from its browser, as well as the browser allows", () => {
  assert.equal(defaultPhoneName("Mozilla/5.0 (Linux; Android 14; Pixel 8 Pro Build/AP1A.240305.019) AppleWebKit/537.36 Chrome/120 Mobile Safari/537.36"), "Pixel 8 Pro");
  assert.equal(defaultPhoneName("Mozilla/5.0 (Linux; Android 13; SM-S911B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Mobile Safari/537.36"), "SM-S911B");
  // Chrome's reduced user agent names no model.
  assert.equal(defaultPhoneName("Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Mobile Safari/537.36"), "Android phone");
  assert.equal(defaultPhoneName("Mozilla/5.0 (Linux; Android 14; wv) AppleWebKit/537.36 Version/4.0 Chrome/120 Mobile Safari/537.36"), "Android phone");
  assert.equal(defaultPhoneName("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15"), "iPhone");
  assert.equal(defaultPhoneName("Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X)"), "iPad");
  assert.equal(defaultPhoneName("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"), "Phone");
});

test("each refusal says what to do next", () => {
  assert.match(pairFailureText(new GazelleError("pairing_refused", "x")), /Pair a phone again/);
  assert.match(pairFailureText(new GazelleError("too_many_attempts", "x")), /Wait a minute/);
  assert.match(pairFailureText(new GazelleError("remote_off", "x")), /Allow phones on this network/);
  assert.match(pairFailureText(new GazelleError("not_connected", "x")), /same network/);
  assert.match(pairFailureText(new Error("boom")), /boom/);
});

test("pairing sends the code and name as typed, trimmed, and says how it went", async () => {
  const sent: [string, string][] = [];
  const ok = await submitPair(" ABCD-EFGH ", " Pixel ", async (code, name) => {
    sent.push([code, name]);
  });
  assert.deepEqual(sent, [["ABCD-EFGH", "Pixel"]]);
  assert.deepEqual(ok, { ok: true, message: "Paired. Opening Gazelle..." });

  const refused = await submitPair("0000-0000", "Pixel", async () => {
    throw new GazelleError("pairing_refused", "That code is not valid now.");
  });
  assert.equal(refused.ok, false);
  assert.match(refused.message, /did not work/);

  let asked = false;
  const empty = await submitPair("  ", "Pixel", async () => {
    asked = true;
  });
  assert.equal(empty.ok, false);
  assert.equal(asked, false, "an empty code is not sent");
});
