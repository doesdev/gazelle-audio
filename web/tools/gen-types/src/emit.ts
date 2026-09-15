// Schema → TypeScript (spec §5). Pure: schemas in, file contents out, so generation and the
// `--check` drift test share one code path. Types follow the server's JSON mapping
// (crates/gazelle-audio-server/src/value.rs): scalars are numbers; byte arrays go in as bytes
// (hex, byte array or Uint8Array) and come out as Uint8Array (the client converts the server's
// hex); struct arrays come out as arrays of objects. Runtime descriptors carry every parsed
// field so the client can convert values and the integration tests can cross-check the server.

import { byteSize, parseField, SCALAR_BYTES, SchemaError, type Field, type Scalar } from "./field.ts";

export interface SchemaJson {
  commands: Record<string, { report_id: string; params?: readonly unknown[]; returns?: readonly unknown[] }>;
  cyclic_reports: Record<string, { report_id?: string; fields: readonly unknown[] }>;
}

export interface SchemaInput {
  family: string;
  /** Repository-relative path named in the generated header. */
  source: string;
  schema: SchemaJson;
  /** The family's `refs/schemas/<family>_topology.json`, when there is one. */
  topology?: unknown;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Enough of the topology's shape that the generated constant can satisfy `Topology`; the Rust tests check its content. */
function checkTopology(family: string, topology: unknown): Record<string, unknown> {
  const fail = (message: string): never => {
    throw new SchemaError(`${family} topology: ${message}`);
  };
  if (!isRecord(topology)) return fail("must be an object");
  if (topology["family"] !== family) fail(`names family ${JSON.stringify(topology["family"])}`);
  for (const key of ["inputs", "outputs", "signalPresent", "availableChannels", "assumptions"]) {
    if (!Array.isArray(topology[key])) fail(`${key} must be a list`);
  }
  const mixers = topology["mixers"];
  if (!isRecord(mixers) || !Number.isInteger(mixers["count"]) || !Number.isInteger(mixers["channels"]) || typeof mixers["command"] !== "string") {
    fail("mixers needs count, channels and command");
  }
  return topology;
}

const ZERO_LENGTH = "zero-length array; see .agent/reference/devices.md";

function pascal(family: string): string {
  return family.charAt(0).toUpperCase() + family.slice(1);
}

function property(name: string): string {
  return /^[A-Za-z_$][\w$]*$/.test(name) ? name : JSON.stringify(name);
}

/** The server's spelling of a report id: `0x` and uppercase hex digits. */
export function reportKey(id: string): string {
  const n = Number.parseInt(id, 16);
  if (!Number.isInteger(n)) throw new Error(`bad report id ${JSON.stringify(id)}`);
  return `0x${n.toString(16).toUpperCase()}`;
}

function range(scalar: Scalar, bits: number): string {
  if (scalar.startsWith("i")) return `${-(2 ** (bits - 1))}..${2 ** (bits - 1) - 1}`;
  return `0..${2 ** bits - 1}`;
}

const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"}`;

function describe(field: Field): string {
  const zero = (count: number) => (count === 0 ? `; ${ZERO_LENGTH}` : "");
  switch (field.kind) {
    case "scalar": {
      const bits = field.bitWidth ?? SCALAR_BYTES[field.scalar] * 8;
      const width = field.bitWidth === undefined ? "" : `, ${plural(field.bitWidth, "bit")}`;
      const fallback = field.default === undefined ? "" : `; default ${field.default}`;
      return `${field.scalar}${width}, ${range(field.scalar, bits)}${fallback}`;
    }
    case "array":
      return `${field.count} × ${field.elem}${zero(field.count)}`;
    case "struct_array":
      return `${field.count} × struct of ${plural(field.count === 0 ? 0 : byteSize(field) / field.count, "byte")}${zero(field.count)}`;
    case "elem_array":
      return `${plural(field.count, "element")} of ${plural(byteSize(field.elem), "byte")}${zero(field.count)}`;
  }
}

type Mode = "input" | "output";

function valueType(field: Field, mode: Mode, indent: number): string {
  switch (field.kind) {
    case "scalar":
      return "number";
    case "array":
      return mode === "input" ? "Bytes" : "Uint8Array";
    case "elem_array":
      return mode === "input" ? "Bytes | readonly Bytes[]" : "Uint8Array";
    case "struct_array":
      return mode === "input" ? "Bytes" : `Array<${objectType(field.fields, mode, indent)}>`;
  }
}

function objectType(fields: readonly Field[], mode: Mode, indent: number): string {
  if (fields.length === 0) return "Record<string, never>";
  const pad = "  ".repeat(indent + 1);
  const lines = fields.map((f) => {
    const optional = mode === "input" && f.kind === "scalar" && f.default !== undefined ? "?" : "";
    return `${pad}/** ${describe(f)} */\n${pad}${property(f.name)}${optional}: ${valueType(f, mode, indent + 1)};`;
  });
  return `{\n${lines.join("\n")}\n${"  ".repeat(indent)}}`;
}

/** A compact, deterministic object literal: `{ "k": v }`, `[a, b]`. */
function literal(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(literal).join(", ")}]`;
  if (value !== null && typeof value === "object") {
    const entries = Object.entries(value);
    return entries.length === 0 ? "{}" : `{ ${entries.map(([k, v]) => `${JSON.stringify(k)}: ${literal(v)}`).join(", ")} }`;
  }
  return JSON.stringify(value);
}

const byName = <T>(entries: [string, T][]) => entries.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));

function familyFile(input: SchemaInput): string {
  const P = pascal(input.family);
  const commands = byName(Object.entries(input.schema.commands)).map(([name, command]) => ({
    name,
    reportId: reportKey(command.report_id),
    params: (command.params ?? []).map((entry) => parseField(entry, `${name}.params`)),
    returns: command.returns === undefined ? null : command.returns.map((entry) => parseField(entry, `${name}.returns`)),
  }));
  const reports = byName(Object.entries(input.schema.cyclic_reports)).map(([id, report]) => ({
    id: reportKey(report.report_id ?? id),
    fields: report.fields.map((entry) => parseField(entry, `cyclic ${id}`)),
  }));

  const out: string[] = [];
  out.push(`// Generated by tools/gen-types from ${input.source}. Do not edit.`, "");
  out.push(`import type { Bytes, FamilySchema, Topology } from "../schema.ts";`, "");
  out.push(`export interface ${P}Commands {`);
  for (const c of commands) {
    out.push(`  ${property(c.name)}: {`);
    out.push(`    params: ${objectType(c.params, "input", 2)};`);
    out.push(`    returns: ${c.returns === null ? "null" : objectType(c.returns, "output", 2)};`);
    out.push("  };");
  }
  out.push("}", "");
  out.push(`export interface ${P}CyclicReports {`);
  for (const r of reports) out.push(`  ${JSON.stringify(r.id)}: ${objectType(r.fields, "output", 1)};`);
  out.push("}", "");
  out.push(`export const ${input.family}Schema = {`, `  "commands": {`);
  for (const c of commands) out.push(`    ${JSON.stringify(c.name)}: ${literal({ reportId: c.reportId, params: c.params, returns: c.returns })},`);
  out.push("  },", `  "cyclic": {`);
  for (const r of reports) out.push(`    ${JSON.stringify(r.id)}: ${literal(r.fields)},`);
  out.push("  },", "} as const satisfies FamilySchema;", "");
  if (input.topology !== undefined) {
    const topology = checkTopology(input.family, input.topology);
    out.push(`/** Routing groups and mixers, extracted from the vendor panels' bytecode (see \`source\` and \`assumptions\`). */`);
    out.push(`export const ${input.family}Topology = ${literal(topology)} as const satisfies Topology;`, "");
  }
  return out.join("\n");
}

function indexFile(inputs: readonly SchemaInput[]): string {
  const out: string[] = ["// Generated by tools/gen-types from refs/schemas/*.json. Do not edit.", ""];
  for (const { family, topology } of inputs) {
    const P = pascal(family);
    const values = topology === undefined ? `${family}Schema` : `${family}Schema, ${family}Topology`;
    out.push(`import { ${values}, type ${P}Commands, type ${P}CyclicReports } from "./${family}.ts";`);
  }
  out.push("", `export type Family = ${inputs.map((i) => JSON.stringify(i.family)).join(" | ")};`, "");
  out.push("export interface FamilyTypes {");
  for (const { family } of inputs) out.push(`  ${family}: { commands: ${pascal(family)}Commands; cyclic: ${pascal(family)}CyclicReports };`);
  out.push("}", "");
  out.push(`export const schemas = { ${inputs.map((i) => `${i.family}: ${i.family}Schema`).join(", ")} } as const;`, "");
  const withTopology = inputs.filter((i) => i.topology !== undefined);
  if (withTopology.length > 0) {
    out.push(`export const topologies = { ${withTopology.map((i) => `${i.family}: ${i.family}Topology`).join(", ")} } as const;`, "");
  }
  return out.join("\n");
}

/** File name → contents for every family, then `index.ts`. Throws `SchemaError` on any unknown type. */
export function generate(inputs: readonly SchemaInput[]): Map<string, string> {
  const files = new Map<string, string>();
  for (const input of inputs) files.set(`${input.family}.ts`, familyFile(input));
  files.set("index.ts", indexFile(inputs));
  return files;
}
