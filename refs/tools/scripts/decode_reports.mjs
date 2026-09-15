#!/usr/bin/env node
// Decode Antelope reports from a USB capture, for hardware sessions.
//
//   gazelle-capture import CAPTURE.pcapng --vid 0x23e5 --pid 0xa2f9 > events.jsonl
//   node refs/tools/scripts/decode_reports.mjs events.jsonl --family quadro [--cyclic] [--field volumes --field line_gains]
//
// Reads the JSON lines of UsbEvent that `import` writes. Host-to-device reports are interrupt OUT
// submits; device-to-host reports are interrupt IN completions. Each carries a 16-byte header
// (cmd, seq, ext2, ext3, little-endian u32) at byte 0 and is zero-padded to the packet size.
// Segmented messages (cmd 8052 out / 8053 in; seq = chunk + 16, ext2 = total, ext3 = offset) are
// reassembled first, and only those have an exact length. Commands are named from
// refs/schemas/<family>_commands.json: 0x70 by payload id, 0x74 and replies by ext2 (and ext3).
// Cyclic reports (0x73, 0x83) are hidden unless --cyclic is given; --field NAME instead prints
// that 0x73 field's bytes whenever they change.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const file = args.find((a) => !a.startsWith("--") && args[args.indexOf(a) - 1] !== "--family" && args[args.indexOf(a) - 1] !== "--field");
const family = args[args.indexOf("--family") + 1];
const showCyclic = args.includes("--cyclic");
const fields = args.flatMap((a, i) => (a === "--field" ? [args[i + 1]] : []));
if (!file || (family !== "quadro" && family !== "studio")) {
  console.error("usage: decode_reports.mjs EVENTS.jsonl --family quadro|studio [--cyclic] [--field NAME]...");
  process.exit(2);
}

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const schema = JSON.parse(readFileSync(join(root, "refs", "schemas", `${family}_commands.json`), "utf8"));
const commands = Object.values(schema.commands);
const hex = (bytes) => Buffer.from(bytes).toString("hex");
const u32 = (b, at) => (b[at] | (b[at + 1] << 8) | (b[at + 2] << 16) | (b[at + 3] << 24)) >>> 0;
const trimZeros = (bytes) => {
  let end = bytes.length;
  while (end > 0 && bytes[end - 1] === 0) end--;
  return bytes.slice(0, end);
};

function nameOf(cmd, ext2, ext3, payload) {
  const request = cmd & 1 ? cmd - 1 : cmd;
  const reply = request !== cmd ? " (reply)" : "";
  const candidates = commands.filter((c) => Number.parseInt(c.report_id, 16) === request);
  if (request === 0x70) {
    const id = payload[0] & 0x3f;
    const c = candidates.find((c) => c.payload_id === id);
    return c ? c.name + reply : `payload ${id}${reply}`;
  }
  const exact = candidates.find((c) => c.ext2 === ext2 && c.ext3 === ext3) ?? candidates.find((c) => c.ext2 === ext2 && (c.name === "get_routing" || c.name === "get_mixer"));
  return exact ? exact.name + reply : `0x${cmd.toString(16)}`;
}

// Byte offsets of 0x73 fields, laid out as the protocol crate reads them: bit fields pack LSB-first,
// whole-byte fields resume at the next byte boundary.
const typeBytes = { ubyte: 1, byte: 1, uint8: 1, int8: 1, char: 1, bool: 1, uint16: 2, int16: 2, short: 2, ushort: 2, uint32: 4, int32: 4, uint: 4, int: 4 };
const layout = new Map();
{
  let bit = 0;
  for (const f of schema.cyclic_reports?.["0x73"]?.fields ?? []) {
    if (f.bit_width) {
      layout.set(f.name, { at: bit / 8, bits: f.bit_width });
      bit += f.bit_width;
    } else {
      bit = Math.ceil(bit / 8) * 8;
      const size = f.size ?? typeBytes[f.type] ?? 1;
      layout.set(f.name, { at: bit / 8, size });
      bit += size * 8;
    }
  }
}
for (const f of fields) if (!layout.has(f)) console.error(`no 0x73 field ${f}; fields: ${[...layout.keys()].join(", ")}`);

const events = readFileSync(file, "utf8").split(/\r?\n/).filter(Boolean).map((l) => JSON.parse(l));
const t0 = events[0]?.ts_ns ?? 0;
const segments = { out: null, in: null };
const lastField = new Map();
let cyclicCount = 0;

for (const e of events) {
  if (e.transfer !== "interrupt" || e.data_len < 16) continue;
  const dir = e.direction;
  if ((dir === "out" && e.stage !== "submit") || (dir === "in" && e.stage !== "complete")) continue;
  const b = e.data;
  let cmd = u32(b, 0);
  const t = ((e.ts_ns - t0) / 1e6).toFixed(0).padStart(7);
  let message = b;
  if (cmd === 8052 || cmd === 8053) {
    const [seq, total, offset] = [u32(b, 4), u32(b, 8), u32(b, 12)];
    const chunk = b.slice(16, seq);
    const state = offset === 0 || segments[dir] === null ? (segments[dir] = { total, bytes: [] }) : segments[dir];
    state.bytes.push(...chunk);
    if (state.bytes.length < state.total) continue;
    message = state.bytes.slice(0, state.total);
    segments[dir] = null;
    cmd = u32(message, 0);
  }
  const [seq, ext2, ext3] = [u32(message, 4), u32(message, 8), u32(message, 12)];
  const payload = message.slice(16);
  const exact = message !== b;
  if (cmd === 0x73 && fields.length > 0) {
    for (const name of fields) {
      const f = layout.get(name);
      if (!f) continue;
      const value = f.bits ? `bits@${f.at}` + hex(payload.slice(Math.floor(f.at), Math.ceil(f.at + f.bits / 8))) : hex(payload.slice(f.at, f.at + f.size));
      if (lastField.get(name) !== value) {
        lastField.set(name, value);
        console.log(`${t} ms  0x73 ${name} = ${value}`);
      }
    }
    continue;
  }
  if ((cmd === 0x73 || cmd === 0x83) && !showCyclic) {
    cyclicCount++;
    continue;
  }
  const name = nameOf(cmd, ext2, ext3, payload);
  const shown = exact ? payload : trimZeros(payload);
  console.log(`${t} ms  ${dir.padEnd(3)} 0x${cmd.toString(16).padStart(2, "0")} ext2=${ext2} ext3=${ext3} seq=${seq} ${exact ? `len=${payload.length}` : `nonzero=${shown.length}`}  ${name}  ${hex(shown)}`);
}
if (!showCyclic && fields.length === 0) console.log(`(${cyclicCount} cyclic reports hidden)`);
