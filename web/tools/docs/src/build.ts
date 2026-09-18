// `pnpm -C web docs`: checks the manual and the cheat sheet under docs/, then prints each to a
// PDF with the Chromium that Playwright installs for the e2e suite.
//
//   docs/dist/gazelle-manual.pdf       title page, contents with page numbers, one chapter per
//                                      page run, numbered figures, a PDF outline
//   docs/dist/gazelle-cheat-sheet.pdf  at most two pages
//
// `pnpm -C web docs:check` runs the checks alone (check.ts). The checks: no em or en dash anywhere in
// README.md or docs/; every relative link, image and anchor resolves; book.json lists every page;
// every image is used; and, when a server binary has been built, the command-line chapter agrees
// with its --help.
//
// Page numbers in the contents come from printing twice: the first PDF's outline (one entry per
// heading) says where each heading landed, and the second print carries those numbers. The
// contents reserve the numbers' width on the first print, so the second lays out the same; the
// build checks that it did.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { chromium, type Browser } from "@playwright/test";

import { checkBook, checkDashes, checkLinks, filesUnder, readBook, type Book } from "./checks.ts";
import { compareCli, findBinary, helpOf } from "./cli.ts";
import { renderMarkdown, type HeadingInfo } from "./markdown.ts";
import { outline, pageCount } from "./pdf.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(HERE, "../../../..");
const DOCS = join(REPO_ROOT, "docs");
const OUT = join(DOCS, "dist");
const CLI_CHAPTER = "manual/16-command-line-and-api.md";
const CHEAT_SHEET_MAX_PAGES = 2;

function fail(heading: string, problems: readonly string[]): never {
  console.error(`\n${heading}:`);
  for (const problem of problems) console.error(`  ${problem}`);
  process.exit(1);
}

/** Runs every check; returns the book when all pass. */
export function runChecks(): Book {
  const book = readBook(DOCS);
  const markdown = [join(REPO_ROOT, "README.md"), ...filesUnder(DOCS, [".md"])];
  const texts = [...markdown, join(DOCS, "book.json")];
  const problems = [...checkDashes(REPO_ROOT, texts), ...checkLinks(REPO_ROOT, markdown), ...checkBook(DOCS, book, markdown)];
  if (problems.length > 0) fail(`The docs have ${problems.length} problem${problems.length === 1 ? "" : "s"}`, problems);
  console.log(`checked ${markdown.length} Markdown files: no dashes, every link and image resolves, the book is complete`);

  const binary = findBinary(REPO_ROOT);
  if (binary === undefined) {
    console.log("command-line check skipped: no gazelle-audio-server binary is built (cargo build -p gazelle-audio-server, or set GAZELLE_BIN)");
  } else {
    const cli = compareCli(helpOf(binary), readFileSync(join(DOCS, CLI_CHAPTER), "utf8"));
    if (cli.length > 0) fail(`docs/${CLI_CHAPTER} disagrees with ${relative(REPO_ROOT, binary)} --help`, cli);
    console.log(`checked docs/${CLI_CHAPTER} against ${relative(REPO_ROOT, binary)} --help`);
  }
  return book;
}

function version(): string {
  const toml = readFileSync(join(REPO_ROOT, "crates/gazelle-audio-server/Cargo.toml"), "utf8");
  return /^version\s*=\s*"([^"]+)"/m.exec(toml)?.[1] ?? "unknown";
}

/** The date the docs last changed in git, so a rebuild of the same commit prints the same page. */
function editionDate(): string {
  const git = spawnSync("git", ["log", "-1", "--format=%cs", "--", "docs", "README.md"], { cwd: REPO_ROOT, encoding: "utf8" });
  const date = git.status === 0 ? git.stdout.trim() : "";
  const [y, m, d] = (date || new Date().toISOString().slice(0, 10)).split("-").map(Number);
  const months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
  return `${d} ${months[(m ?? 1) - 1]} ${y}`;
}

/** @font-face rules for the fonts the app bundles, when they are installed. */
function fontFaces(): string {
  const files = join(REPO_ROOT, "web/apps/web/node_modules/@fontsource-variable");
  const faces: [string, string, string][] = [
    ["Inter Variable", "inter/files/inter-latin-wght-normal.woff2", "normal"],
    ["Inter Variable", "inter/files/inter-latin-wght-italic.woff2", "italic"],
    ["Josefin Sans Variable", "josefin-sans/files/josefin-sans-latin-wght-normal.woff2", "normal"],
  ];
  return faces
    .filter(([, file]) => existsSync(join(files, file)))
    .map(([family, file, style]) => `@font-face { font-family: "${family}"; font-style: ${style}; font-weight: 100 900; src: url("${pathToFileURL(join(files, file)).href}") format("woff2"); }`)
    .join("\n");
}

const escapeHtml = (text: string): string => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

interface Chapter {
  file: string;
  html: string;
  title: string;
  headings: HeadingInfo[];
}

/** The id a chapter's anchors start with: its file name without the number or extension. */
const chapterId = (file: string): string => file.replace(/^.*\//, "").replace(/\.md$/, "");

function renderChapters(book: Book): Chapter[] {
  const ids = new Map(book.chapters.map((file) => [resolve(DOCS, file), chapterId(file)]));
  return book.chapters.map((file, index) => {
    const path = resolve(DOCS, file);
    const rendered = renderMarkdown(readFileSync(path, "utf8"), {
      idPrefix: chapterId(file),
      chapter: index + 1,
      resolveLink: (href) => {
        const [target = "", fragment] = href.split("#");
        const id = target === "" ? chapterId(file) : ids.get(resolve(dirname(path), decodeURIComponent(target)));
        if (id === undefined) return undefined;
        return fragment ? `#${id}--${fragment}` : `#${id}`;
      },
      imageSrc: (href) => pathToFileURL(resolve(dirname(path), decodeURIComponent(href))).href,
    });
    if (!rendered.title) throw new Error(`${file} has no level-one heading`);
    return { file, html: rendered.html, title: rendered.title, headings: rendered.headings };
  });
}

function manualHtml(book: Book, chapters: readonly Chapter[], pages: ReadonlyMap<string, number> | undefined): string {
  const page = (id: string) => (pages === undefined ? "888" : String(pages.get(id) ?? "?"));
  const toc = chapters
    .map((chapter) => {
      const [top, ...rest] = chapter.headings.filter((h) => h.depth <= 2);
      if (top === undefined) return "";
      const row = (h: HeadingInfo, kind: string) =>
        `<li class="${kind}"><a href="#${h.id}"><span class="n">${h.number ?? ""}</span><span class="t">${escapeHtml(h.text)}</span><span class="dots"></span><span class="p">${page(h.id)}</span></a></li>`;
      return row(top, "chapter") + rest.map((h) => row(h, "section")).join("");
    })
    .join("\n");
  const css = readFileSync(join(HERE, "manual.css"), "utf8");
  return `<!doctype html>
<html lang="en-GB"><head><meta charset="utf-8"><title>${escapeHtml(book.title)}</title>
<style>${fontFaces()}\n${css}</style></head>
<body>
<section class="title-page">
  <div class="brand">Gazelle</div>
  <div class="rule"></div>
  <div class="subtitle">${escapeHtml(book.subtitle)}</div>
  <div class="for">For the Antelope Audio Zen Quadro Synergy Core and Zen Studio+</div>
  <div class="edition">Version ${escapeHtml(version())}, ${escapeHtml(editionDate())}</div>
  <div class="first"><strong>Before anything else:</strong> read chapter 2, Safety. Keep monitors and headphones low whenever you try something new, and know where your physical mute is.</div>
  <div class="fine">Gazelle is independent software, written from the protocol the devices speak. It is not affiliated with, endorsed by or supported by Antelope Audio. Antelope Audio, Zen Quadro, Zen Studio and Synergy Core are trademarks of their owners and are used here only to say which hardware Gazelle works with. Gazelle is free software under the MIT licence and comes with no warranty.</div>
</section>
<nav class="toc"><p class="toc-title">Contents</p><ol>
${toc}
</ol></nav>
${chapters.map((c) => `<article class="chapter">\n${c.html}</article>`).join("\n")}
</body></html>`;
}

async function printPdf(browser: Browser, html: string, htmlPath: string): Promise<Uint8Array> {
  writeFileSync(htmlPath, html);
  const page = await browser.newPage();
  try {
    await page.goto(pathToFileURL(htmlPath).href, { waitUntil: "load" });
    await page.evaluate(() => document.fonts.ready.then(() => undefined));
    const images = await page.evaluate(() => [...document.images].filter((img) => !img.complete || img.naturalWidth === 0).map((img) => img.getAttribute("src")));
    if (images.length > 0) throw new Error(`images did not load: ${images.join(", ")}`);
    return await page.pdf({ format: "A4", preferCSSPageSize: true, printBackground: true, outline: true, tagged: true });
  } finally {
    await page.close();
  }
}

/** Where each chapter and section heading landed, by matching the outline to the headings. */
function headingPages(pdf: Uint8Array, chapters: readonly Chapter[]): Map<string, number> {
  const entries = outline(pdf);
  const squash = (text: string) => text.replace(/\s+/g, "").toLowerCase();
  const pages = new Map<string, number>();
  let at = 0;
  for (const heading of chapters.flatMap((c) => c.headings)) {
    const want = squash(heading.text);
    while (at < entries.length && !squash(entries[at]!.title).endsWith(want)) at += 1;
    if (at === entries.length) throw new Error(`the PDF outline has no entry for the heading "${heading.text}"`);
    pages.set(heading.id, entries[at]!.page);
    at += 1;
  }
  return pages;
}

async function buildManual(browser: Browser, book: Book): Promise<number> {
  const chapters = renderChapters(book);
  const htmlPath = join(OUT, "gazelle-manual.html");
  let pages = headingPages(await printPdf(browser, manualHtml(book, chapters, undefined), htmlPath), chapters);
  for (let attempt = 0; attempt < 3; attempt++) {
    const pdf = await printPdf(browser, manualHtml(book, chapters, pages), htmlPath);
    const landed = headingPages(pdf, chapters);
    const moved = [...landed].filter(([id, p]) => pages.get(id) !== p);
    if (moved.length === 0) {
      writeFileSync(join(OUT, "gazelle-manual.pdf"), pdf);
      return pageCount(pdf);
    }
    pages = landed;
  }
  throw new Error("the contents' page numbers did not settle after three prints");
}

async function buildCheatSheet(browser: Browser, book: Book): Promise<number> {
  const path = resolve(DOCS, book.cheatSheet);
  const rendered = renderMarkdown(readFileSync(path, "utf8"), {
    idPrefix: "cheat",
    resolveLink: () => undefined,
    imageSrc: (href) => pathToFileURL(resolve(dirname(path), href)).href,
  });
  // The first paragraph under the title spans both columns; each h2 and what follows it is kept
  // together in a section, so no heading is stranded at the foot of a column.
  const [head = "", ...sections] = rendered.html.split(/(?=<h2 )/);
  const top = /^\s*(<h1[\s\S]*?<\/h1>)\s*(<p>[\s\S]*?<\/p>)?/.exec(head);
  const title = top === null ? "" : `${top[1]}${top[2]?.replace("<p>", '<p class="lede">') ?? ""}`;
  const rest = top === null ? head : head.slice(top[0].length);
  const css = readFileSync(join(HERE, "cheat-sheet.css"), "utf8");
  const html = `<!doctype html>
<html lang="en-GB"><head><meta charset="utf-8"><title>${escapeHtml(rendered.title)}</title>
<style>${fontFaces()}\n${css}</style></head>
<body>${title}<main>${rest}${sections.map((s) => `<section>${s}</section>`).join("\n")}</main></body></html>`;
  const pdf = await printPdf(browser, html, join(OUT, "gazelle-cheat-sheet.html"));
  const count = pageCount(pdf);
  if (count > CHEAT_SHEET_MAX_PAGES) fail("The cheat sheet is too long", [`it prints on ${count} pages; it must fit on ${CHEAT_SHEET_MAX_PAGES}`]);
  writeFileSync(join(OUT, "gazelle-cheat-sheet.pdf"), pdf);
  return count;
}

async function main(): Promise<void> {
  const book = runChecks();
  mkdirSync(OUT, { recursive: true });
  const browser = await chromium.launch();
  try {
    const manual = await buildManual(browser, book);
    const sheet = await buildCheatSheet(browser, book);
    const out = relative(REPO_ROOT, OUT).split(sep).join("/");
    console.log(`wrote ${out}/gazelle-manual.pdf (${manual} pages) and ${out}/gazelle-cheat-sheet.pdf (${sheet} page${sheet === 1 ? "" : "s"})`);
  } finally {
    await browser.close();
  }
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
