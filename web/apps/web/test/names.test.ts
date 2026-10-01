import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { topologies } from "gazelle-audio-client";

import { DISPLAY_WORDS, groupName, properCase } from "../src/store/names.ts";

/** The cases the server's tests read too, so the two cannot show a name differently. */
const SHARED_CASES = JSON.parse(readFileSync(new URL("../../../../refs/fixtures/display_names.json", import.meta.url), "utf8")) as { cases: [string, string][] };

test("the devices' capitals are shown in plain case, with the acronyms kept, as the shared cases say", () => {
  assert.equal(SHARED_CASES.cases.length > 30, true, "the file has its cases");
  for (const [device, shown] of SHARED_CASES.cases) assert.equal(properCase(device), shown, device);
});

test("doing it twice changes nothing", () => {
  for (const [device, shown] of SHARED_CASES.cases) assert.equal(properCase(shown), shown, device);
});

test("every word either model spells its groups with comes out in plain case or is an acronym", () => {
  const acronyms = new Set(["USB", "ADAT", "AFX", "TB", "HP", "A", "B", "L", "R"]);
  for (const topology of Object.values(topologies)) {
    for (const group of [...topology.inputs, ...topology.outputs]) {
      for (const word of group.name.split(/[ /]/)) {
        const letters = word.replace(/\d+$/, "");
        if (letters === "") continue;
        assert.equal(acronyms.has(letters) || DISPLAY_WORDS[letters] !== undefined, true, `${group.name}: ${word}`);
      }
      for (const word of groupName(group).split(" ")) assert.equal(word === "S/PDIF" || acronyms.has(word.replace(/\d+$/, "")) || !/^[A-Z]{2,}/.test(word), true, `${groupName(group)}: ${word}`);
    }
  }
});
