// Just enough of a PDF reader to learn two things from the PDFs Chromium writes: how many pages
// there are, and which page each entry of the document outline (one per heading) points at. The
// table of contents is printed with those page numbers, which is why the manual is printed twice.
//
// Chromium's PDFs keep their dictionaries as plain objects (no object streams), which this relies
// on; `outline` fails loudly rather than guessing if that ever stops being true.

interface PdfObjects {
  get(id: number): string | undefined;
}

function objects(pdf: Uint8Array): PdfObjects {
  const text = Buffer.from(pdf).toString("latin1");
  const table = new Map<number, string>();
  const pattern = /(\d+) 0 obj\b([\s\S]*?)endobj/g;
  for (let match = pattern.exec(text); match !== null; match = pattern.exec(text)) {
    const body = match[2] ?? "";
    // Only the dictionary before any stream matters here.
    const stream = body.indexOf("stream");
    table.set(Number(match[1]), stream < 0 ? body : body.slice(0, stream));
  }
  if (text.includes("/ObjStm")) throw new Error("this PDF keeps objects in object streams, which the page counter cannot read");
  return { get: (id) => table.get(id) };
}

const ref = (dict: string, key: string): number | undefined => {
  const match = new RegExp(`/${key}\\s+(\\d+)\\s+0\\s+R`).exec(dict);
  return match ? Number(match[1]) : undefined;
};

/** The page objects in reading order. */
function pageOrder(pdf: PdfObjects, catalog: string): number[] {
  const root = ref(catalog, "Pages");
  if (root === undefined) throw new Error("the PDF has no page tree");
  const out: number[] = [];
  const walk = (id: number) => {
    const node = pdf.get(id);
    if (node === undefined) throw new Error(`page tree object ${id} is missing`);
    if (/\/Type\s*\/Pages\b/.test(node)) {
      const kids = /\/Kids\s*\[([^\]]*)\]/.exec(node)?.[1] ?? "";
      for (const kid of kids.matchAll(/(\d+)\s+0\s+R/g)) walk(Number(kid[1]));
    } else out.push(id);
  };
  walk(root);
  return out;
}

function catalogOf(pdf: Uint8Array, table: PdfObjects): string {
  const text = Buffer.from(pdf).toString("latin1");
  const rootId = /\/Root\s+(\d+)\s+0\s+R/.exec(text)?.[1];
  const catalog = rootId === undefined ? undefined : table.get(Number(rootId));
  if (catalog === undefined) throw new Error("the PDF has no catalog");
  return catalog;
}

export function pageCount(pdf: Uint8Array): number {
  const table = objects(pdf);
  return pageOrder(table, catalogOf(pdf, table)).length;
}

/** A PDF string, literal `(…)` or hex `<FEFF…>`, as text. */
function pdfString(dict: string, key: string): string {
  const hex = new RegExp(`/${key}\\s*<([0-9A-Fa-f\\s]*)>`).exec(dict);
  if (hex?.[1] !== undefined) {
    const bytes = Buffer.from(hex[1].replace(/\s/g, ""), "hex");
    if (bytes[0] === 0xfe && bytes[1] === 0xff) {
      let out = "";
      for (let i = 2; i + 1 < bytes.length; i += 2) out += String.fromCharCode((bytes[i]! << 8) | bytes[i + 1]!);
      return out;
    }
    return bytes.toString("latin1");
  }
  const start = dict.search(new RegExp(`/${key}\\s*\\(`));
  if (start < 0) return "";
  let i = dict.indexOf("(", start) + 1;
  let depth = 1;
  let out = "";
  while (i < dict.length && depth > 0) {
    const c = dict[i]!;
    if (c === "\\") {
      const next = dict[i + 1] ?? "";
      const escapes: Record<string, string> = { n: "\n", r: "\r", t: "\t", b: "\b", f: "\f", "(": "(", ")": ")", "\\": "\\" };
      if (/[0-7]/.test(next)) {
        const octal = /^[0-7]{1,3}/.exec(dict.slice(i + 1))![0];
        out += String.fromCharCode(parseInt(octal, 8));
        i += 1 + octal.length;
        continue;
      }
      out += escapes[next] ?? next;
      i += 2;
      continue;
    }
    if (c === "(") depth += 1;
    if (c === ")") depth -= 1;
    if (depth > 0) out += c;
    i += 1;
  }
  return out;
}

export interface OutlineEntry {
  title: string;
  /** 1-based page number. */
  page: number;
  depth: number;
}

/** Every outline entry, depth first, which is document order: one per heading. */
export function outline(pdf: Uint8Array): OutlineEntry[] {
  const table = objects(pdf);
  const catalog = catalogOf(pdf, table);
  const pages = pageOrder(table, catalog);
  const root = ref(catalog, "Outlines");
  if (root === undefined) return [];
  const out: OutlineEntry[] = [];
  const walk = (id: number | undefined, depth: number) => {
    while (id !== undefined) {
      const item = table.get(id);
      if (item === undefined) throw new Error(`outline object ${id} is missing`);
      const dest = /\/Dest\s*\[\s*(\d+)\s+0\s+R/.exec(item)?.[1];
      if (dest === undefined) throw new Error(`outline entry ${id} has no page destination`);
      const index = pages.indexOf(Number(dest));
      if (index < 0) throw new Error(`outline entry ${id} points at object ${dest}, which is not a page`);
      out.push({ title: pdfString(item, "Title"), page: index + 1, depth });
      walk(ref(item, "First"), depth + 1);
      id = ref(item, "Next");
    }
  };
  walk(ref(table.get(root) ?? "", "First"), 1);
  return out;
}
