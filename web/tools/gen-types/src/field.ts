// The schema field grammar, ported from crates/gazelle-audio-protocol/src/field.rs so the Rust and
// TypeScript readings of refs/schemas/*.json stay independent. Unlike the Rust parser,
// which only has to load what exists, anything unrecognised here throws: a generated type must
// never guess.

export type Scalar = "u8" | "i8" | "u16" | "i16" | "u32" | "i32";

export type Field =
  | { kind: "scalar"; name: string; scalar: Scalar; bitWidth?: number; default?: number }
  | { kind: "array"; name: string; elem: Scalar; count: number }
  | { kind: "struct_array"; name: string; fields: Field[]; count: number }
  | { kind: "elem_array"; name: string; elem: Field; count: number };

export class SchemaError extends Error {
  override name = "SchemaError";
}

const SCALARS: Record<string, Scalar> = {
  ubyte: "u8", uint8: "u8", u8: "u8",
  byte: "i8", int8: "i8", i8: "i8",
  uint16: "u16", u16: "u16", ushort: "u16",
  // ctypes' c_short is signed; field.rs groups bare "short" with int16 for that reason.
  short: "i16", int16: "i16", i16: "i16",
  uint32: "u32", u32: "u32",
  int32: "i32", i32: "i32",
};

export const SCALAR_BYTES: Record<Scalar, number> = { u8: 1, i8: 1, u16: 2, i16: 2, u32: 4, i32: 4 };

export function parseScalar(type: string): Scalar | undefined {
  return Object.hasOwn(SCALARS, type.trim()) ? SCALARS[type.trim()] : undefined;
}

function fail(where: string, message: string): never {
  throw new SchemaError(`${where}: ${message}`);
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function count(where: string, spec: Record<string, unknown>): number {
  const n = spec["count"] ?? 1;
  if (typeof n !== "number" || !Number.isInteger(n) || n < 0) fail(where, `count must be a non-negative integer, got ${JSON.stringify(n)}`);
  return n;
}

function optionalInteger(where: string, label: string, value: unknown): number | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "number" || !Number.isInteger(value)) fail(where, `${label} must be an integer, got ${JSON.stringify(value)}`);
  return value;
}

/** One field entry: `{name, type, bit_width?, default?}` or `[name, type, bit_width?, default?]`. */
export function parseField(entry: unknown, where: string): Field {
  let name: unknown, type: unknown, bitWidth: unknown, defaultValue: unknown;
  if (isObject(entry)) {
    ({ name, type, bit_width: bitWidth, default: defaultValue } = entry);
  } else if (Array.isArray(entry)) {
    [name, type, bitWidth, defaultValue] = entry;
  } else {
    fail(where, `a field entry must be an object or a list, got ${JSON.stringify(entry)}`);
  }
  if (typeof name !== "string") fail(where, `field name must be a string, got ${JSON.stringify(name)}`);
  const at = name ? `${where}.${name}` : where;
  if (type === undefined) fail(at, "field has no type");
  return parseType(at, name, type, optionalInteger(at, "bit_width", bitWidth), optionalInteger(at, "default", defaultValue));
}

function parseType(where: string, name: string, spec: unknown, bitWidth?: number, defaultValue?: number): Field {
  if (typeof spec === "string") {
    const text = spec.trim();
    // Struct and element arrays are stored as JSON strings inside `type`.
    if (text.startsWith("{")) {
      let inner: unknown;
      try {
        inner = JSON.parse(text);
      } catch (e) {
        fail(where, `type is not valid JSON: ${(e as Error).message}`);
      }
      return parseType(where, name, inner, bitWidth, defaultValue);
    }
    if (text.includes("*")) {
      const parts = text.split("*").map((p) => p.trim());
      const [left = "", right = ""] = parts;
      if (parts.length !== 2) fail(where, `inline array ${JSON.stringify(spec)} must be "type * N" or "N * type"`);
      const digits = /^\d+$/;
      const elem = parseScalar(left) ?? parseScalar(right);
      const n = digits.test(right) ? Number(right) : digits.test(left) ? Number(left) : undefined;
      if (elem === undefined || n === undefined) fail(where, `unrecognised inline array ${JSON.stringify(spec)}`);
      return { kind: "array", name, elem, count: n };
    }
    const scalar = parseScalar(text);
    if (scalar === undefined) fail(where, `unrecognised type ${JSON.stringify(spec)}`);
    const field: Field = { kind: "scalar", name, scalar };
    if (bitWidth !== undefined) field.bitWidth = bitWidth;
    if (defaultValue !== undefined) field.default = defaultValue;
    return field;
  }
  if (isObject(spec)) {
    if ("fields" in spec) {
      const fields = spec["fields"];
      if (!Array.isArray(fields)) fail(where, "fields must be a list");
      return { kind: "struct_array", name, count: count(where, spec), fields: fields.map((f) => parseField(f, where)) };
    }
    if ("elem_type" in spec) {
      return { kind: "elem_array", name, count: count(where, spec), elem: parseType(where, "", spec["elem_type"]) };
    }
  }
  fail(where, `unrecognised type ${JSON.stringify(spec)}`);
}

/** Total width in bits; a bit-packed scalar counts only its `bitWidth`. */
export function bitLength(field: Field): number {
  switch (field.kind) {
    case "scalar":
      return field.bitWidth ?? SCALAR_BYTES[field.scalar] * 8;
    case "array":
      return SCALAR_BYTES[field.elem] * field.count * 8;
    case "struct_array":
      return field.fields.reduce((bits, f) => bits + bitLength(f), 0) * field.count;
    case "elem_array":
      return bitLength(field.elem) * field.count;
  }
}

/** Serialized size in bytes, as `Field::size` computes it: struct elements pack bit by bit. */
export function byteSize(field: Field): number {
  switch (field.kind) {
    case "scalar":
      return field.bitWidth === undefined ? SCALAR_BYTES[field.scalar] : Math.ceil(field.bitWidth / 8);
    case "array":
      return SCALAR_BYTES[field.elem] * field.count;
    case "struct_array":
      return Math.ceil(field.fields.reduce((bits, f) => bits + bitLength(f), 0) / 8) * field.count;
    case "elem_array":
      return byteSize(field.elem) * field.count;
  }
}
