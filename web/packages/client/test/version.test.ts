import { test } from "node:test";
import assert from "node:assert/strict";

import { API_PATH } from "../src/index.ts";

test("the client targets the server's v1 API", () => {
  assert.equal(API_PATH, "/api/v1");
});
