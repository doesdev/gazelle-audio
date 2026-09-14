import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { byteSize, parseField, SchemaError } from "../src/field.ts";

const schema = (family: string): { commands: Record<string, { params?: object[]; returns?: object[] }>; cyclic_reports: Record<string, { fields: object[] }> } =>
  JSON.parse(readFileSync(new URL(`../../../../refs/schemas/${family}_commands.json`, import.meta.url), "utf8"));

test("scalar names follow field.rs, with bare short signed", () => {
  const expected = { ubyte: "u8", uint8: "u8", u8: "u8", byte: "i8", int8: "i8", i8: "i8", uint16: "u16", u16: "u16", ushort: "u16", short: "i16", int16: "i16", i16: "i16", uint32: "u32", u32: "u32", int32: "i32", i32: "i32" };
  for (const [type, scalar] of Object.entries(expected)) {
    assert.deepEqual(parseField({ name: "x", type }, "t"), { kind: "scalar", name: "x", scalar }, type);
  }
});

test("dict and list entries carry bit width and default", () => {
  assert.deepEqual(parseField({ name: "density", type: "ubyte", bit_width: 8, default: 100 }, "t"), { kind: "scalar", name: "density", scalar: "u8", bitWidth: 8, default: 100 });
  assert.deepEqual(parseField(["mute", "ubyte", 1], "t"), { kind: "scalar", name: "mute", scalar: "u8", bitWidth: 1 });
  assert.deepEqual(parseField(["density", "ubyte", 8, 100], "t"), { kind: "scalar", name: "density", scalar: "u8", bitWidth: 8, default: 100 });
});

test("inline arrays in both spellings", () => {
  for (const type of ["ubyte * 2", "ubyte*2", "2 * ubyte", "2*uint8"]) {
    assert.deepEqual(parseField({ name: "a", type }, "t"), { kind: "array", name: "a", elem: "u8", count: 2 }, type);
  }
  assert.equal(byteSize(parseField({ name: "a", type: "20 * int32" }, "t")), 80);
});

test("struct arrays pack their fields bit by bit, given as objects or JSON strings", () => {
  const volumes = '{"fields": [["volume", "ubyte", 8], ["mute", "ubyte", 1], ["dim_on", "ubyte", 1], ["mono", "ubyte", 1], ["trim", "ubyte", 5]], "count": 6}';
  const field = parseField({ name: "volumes", type: volumes }, "t");
  assert.equal(field.kind, "struct_array");
  assert.equal(field.kind === "struct_array" && field.count, 6);
  assert.equal(byteSize(field), 12, "16 bits per element, 2 bytes, times 6");
  assert.deepEqual(parseField({ name: "s", type: { fields: [["data", "ubyte * 304"]] } }, "t"), {
    kind: "struct_array",
    name: "s",
    count: 1,
    fields: [{ kind: "array", name: "data", elem: "u8", count: 304 }],
  });
});

test("element arrays nest a type, including an uncounted struct", () => {
  const slots = parseField({ name: "slots", type: { elem_type: { fields: [["type", "ubyte"], ["inst", "ubyte"]] }, count: 8 } }, "t");
  assert.deepEqual(slots, {
    kind: "elem_array",
    name: "slots",
    count: 8,
    elem: { kind: "struct_array", name: "", count: 1, fields: [{ kind: "scalar", name: "type", scalar: "u8" }, { kind: "scalar", name: "inst", scalar: "u8" }] },
  });
  assert.equal(byteSize(slots), 16);
  assert.equal(byteSize(parseField({ name: "b", type: '{"elem_type": "ubyte * 2", "count": 32}' }, "t")), 64);
});

test("anything unrecognised fails, naming where", () => {
  const bad: unknown[] = [
    { name: "f", type: "float" },
    { name: "f", type: "ubyte * many" },
    { name: "f", type: "ubyte * 2 * 3" },
    { name: "f", type: { count: 2 } },
    { name: "f", type: { fields: "nope" } },
    { name: "f", type: { fields: [], count: 1.5 } },
    { name: "f", type: 7 },
    { name: "f" },
    ["f"],
    "ubyte",
    { name: "f", type: "ubyte", bit_width: "3" },
  ];
  for (const entry of bad) {
    assert.throws(() => parseField(entry, "set_x.params"), (e: unknown) => e instanceof SchemaError && e.message.includes("set_x.params"), JSON.stringify(entry));
  }
});

test("every field of both schemas parses, and its size matches the schema's", () => {
  for (const family of ["quadro", "studio"]) {
    const s = schema(family);
    let checked = 0;
    const check = (where: string, entry: object) => {
      const field = parseField(entry, where);
      const size = (entry as { size?: number }).size;
      if (size !== undefined) {
        assert.equal(byteSize(field), size, where);
        checked += 1;
      }
    };
    for (const [name, command] of Object.entries(s.commands)) {
      for (const entry of command.params ?? []) check(`${family} ${name}.params`, entry);
      for (const entry of command.returns ?? []) check(`${family} ${name}.returns`, entry);
    }
    for (const [id, report] of Object.entries(s.cyclic_reports)) {
      for (const entry of report.fields) check(`${family} cyclic ${id}`, entry);
    }
    assert.ok(checked > 10, `${family}: sizes were checked (${checked})`);
  }
});
