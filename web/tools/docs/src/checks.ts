// The checks the docs build refuses to print without. Each returns problems as sentences; an
// empty list means the check passed.
//
// - No em dash (U+2014) or en dash (U+2013) anywhere in the documents: the project's house rule.
// - Every relative link and image resolves, and a link's #anchor names a heading in its target.
// - The book lists every page under docs/manual exactly once, and every image is used.

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, extname, join, relative, resolve, sep } from "node:path";

import { headingsOf, targetsOf } from "./markdown.ts";

export const EM_DASH = "\u2014";
export const EN_DASH = "\u2013";

export interface DashFinding {
  line: number;
  column: number;
  dash: "em dash (U+2014)" | "en dash (U+2013)";
}

export function findDashes(text: string): DashFinding[] {
  const out: DashFinding[] = [];
  text.split("\n").forEach((line, index) => {
    for (let column = 0; column < line.length; column++) {
      const c = line[column];
      if (c === EM_DASH) out.push({ line: index + 1, column: column + 1, dash: "em dash (U+2014)" });
      else if (c === EN_DASH) out.push({ line: index + 1, column: column + 1, dash: "en dash (U+2013)" });
    }
  });
  return out;
}

/** Every file under `dir` whose extension is one of `extensions`, recursively. */
export function filesUnder(dir: string, extensions: readonly string[]): string[] {
  if (!existsSync(dir)) return [];
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      if (name === "dist" || name === "node_modules") continue;
      out.push(...filesUnder(path, extensions));
    } else if (extensions.includes(extname(name).toLowerCase())) out.push(path);
  }
  return out.sort();
}

export function checkDashes(root: string, files: readonly string[]): string[] {
  const problems: string[] = [];
  for (const file of files) {
    for (const found of findDashes(readFileSync(file, "utf8"))) {
      problems.push(`${relative(root, file)}:${found.line}:${found.column}: an ${found.dash}; use a comma, colon, semicolon, brackets or "to" instead`);
    }
  }
  return problems;
}

const EXTERNAL = /^(https?:|mailto:)/;

/** Relative links and images in `files` that lead nowhere, and anchors no heading carries. */
export function checkLinks(root: string, files: readonly string[]): string[] {
  const problems: string[] = [];
  const anchors = new Map<string, Set<string>>();
  const anchorsOf = (file: string) => {
    let known = anchors.get(file);
    if (known === undefined) {
      known = new Set(headingsOf(readFileSync(file, "utf8")).map((h) => h.slug));
      anchors.set(file, known);
    }
    return known;
  };
  for (const file of files) {
    const where = relative(root, file);
    for (const target of targetsOf(readFileSync(file, "utf8"))) {
      if (EXTERNAL.test(target.href)) continue;
      const [path = "", fragment] = target.href.split("#");
      const resolved = path === "" ? file : resolve(dirname(file), decodeURIComponent(path));
      if (!existsSync(resolved)) {
        problems.push(`${where}:${target.line}: ${target.kind} target ${target.href} does not exist`);
        continue;
      }
      if (fragment !== undefined && fragment !== "") {
        if (!resolved.endsWith(".md")) problems.push(`${where}:${target.line}: ${target.href} has an anchor but is not a Markdown page`);
        else if (!anchorsOf(resolved).has(fragment)) problems.push(`${where}:${target.line}: ${target.href}: ${relative(root, resolved)} has no heading with the anchor #${fragment}`);
      }
    }
  }
  return problems;
}

export interface Book {
  title: string;
  subtitle: string;
  chapters: string[];
  cheatSheet: string;
}

export function readBook(docs: string): Book {
  return JSON.parse(readFileSync(join(docs, "book.json"), "utf8")) as Book;
}

/** The book and the files on disk agree, and every image is shown somewhere. */
export function checkBook(docs: string, book: Book, markdownFiles: readonly string[]): string[] {
  const problems: string[] = [];
  const listed = new Set<string>();
  for (const chapter of book.chapters) {
    if (listed.has(chapter)) problems.push(`book.json lists ${chapter} twice`);
    listed.add(chapter);
    if (!existsSync(join(docs, chapter))) problems.push(`book.json lists ${chapter}, which does not exist`);
  }
  for (const file of filesUnder(join(docs, "manual"), [".md"])) {
    const name = relative(docs, file).split(sep).join("/");
    if (!listed.has(name)) problems.push(`docs/${name} is not in book.json, so the PDF would leave it out`);
  }
  if (!existsSync(join(docs, book.cheatSheet))) problems.push(`book.json names the cheat sheet ${book.cheatSheet}, which does not exist`);
  const used = new Set<string>();
  for (const file of markdownFiles) {
    for (const target of targetsOf(readFileSync(file, "utf8"))) {
      if (target.kind === "image" && !EXTERNAL.test(target.href)) used.add(resolve(dirname(file), decodeURIComponent(target.href)));
    }
  }
  for (const image of filesUnder(join(docs, "images"), [".png", ".jpg", ".jpeg", ".svg", ".webp"])) {
    if (!used.has(resolve(image))) problems.push(`docs/images/${relative(join(docs, "images"), image)} is not shown anywhere`);
  }
  return problems;
}
