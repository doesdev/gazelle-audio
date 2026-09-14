// Byte values between the caller's shapes and the server's JSON (server src/value.rs): the server
// sends byte arrays as lowercase hex and accepts hex or arrays of 0-255, with element arrays
// also accepted one entry per element. The client sends Uint8Array as hex and turns received
// hex into Uint8Array, guided by the generated field descriptors.

import type { FieldDescriptor } from "./schema.ts";

export function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Uint8Array);
}

export function toHex(bytes: Uint8Array): string {
  let hex = "";
  for (const byte of bytes) hex += byte.toString(16).padStart(2, "0");
  return hex;
}

/** Parses hex, optionally `0x`-prefixed. */
export function fromHex(hex: string): Uint8Array {
  const text = hex.startsWith("0x") || hex.startsWith("0X") ? hex.slice(2) : hex;
  if (text.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(text)) throw new TypeError(`not a hex byte string: ${JSON.stringify(hex)}`);
  const bytes = new Uint8Array(text.length / 2);
  for (let i = 0; i < bytes.length; i++) bytes[i] = Number.parseInt(text.slice(2 * i, 2 * i + 2), 16);
  return bytes;
}

function wireBytes(value: unknown): unknown {
  return value instanceof Uint8Array ? toHex(value) : value;
}

/** Request arguments as the server accepts them; `undefined` values are omitted so defaults apply. */
export function encodeArgs(params: readonly FieldDescriptor[], args: Readonly<Record<string, unknown>> | undefined): Record<string, unknown> | undefined {
  if (args === undefined) return undefined;
  const out: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(args)) {
    if (value === undefined) continue;
    const field = params.find((f) => f.name === name);
    const perElement = field?.kind === "elem_array" && Array.isArray(value) && value.some((v) => typeof v !== "number");
    out[name] = perElement ? (value as unknown[]).map(wireBytes) : wireBytes(value);
  }
  return out;
}

/** Received fields with byte arrays as Uint8Array, recursing into struct arrays. */
export function decodeFields(fields: readonly FieldDescriptor[], values: Readonly<Record<string, unknown>>): Record<string, unknown> {
  const out: Record<string, unknown> = { ...values };
  for (const field of fields) {
    const value = values[field.name];
    if ((field.kind === "array" || field.kind === "elem_array") && typeof value === "string") {
      out[field.name] = fromHex(value);
    } else if (field.kind === "struct_array" && Array.isArray(value)) {
      const inner = field.fields;
      out[field.name] = value.map((element: unknown) => (isObject(element) ? decodeFields(inner, element) : element));
    }
  }
  return out;
}
