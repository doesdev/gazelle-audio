// Markdown to HTML for the manual and the cheat sheet, on `marked`. What it adds to plain
// Markdown is only what a printed book needs and a Markdown file on GitHub can still read:
//
// - headings carry GitHub's own anchors, so a link that works on GitHub works in the PDF;
// - a chapter's h1 and h2 are numbered ("Chapter 3", "3.2"), and nothing else is;
// - an image alone in its paragraph becomes a numbered figure, its alt text the caption;
// - a link to another page of the book becomes a link inside the one PDF document;
// - a quote that opens with "**Warning.**" or "**Note.**" becomes a callout of that kind.

import { Marked, type Token, type Tokens } from "marked";

/** GitHub's heading anchor: lower case, punctuation dropped, spaces as hyphens. */
export function slugify(text: string): string {
  return text
    .toLowerCase()
    .trim()
    .replace(/[^\p{L}\p{N}\p{M}\p{Pc} -]/gu, "")
    .replace(/ /g, "-");
}

/** Makes each slug unique within one document, as GitHub does: `a`, `a-1`, `a-2`. */
export class Slugger {
  #seen = new Map<string, number>();
  slug(text: string): string {
    const base = slugify(text);
    const count = this.#seen.get(base);
    if (count === undefined) {
      this.#seen.set(base, 0);
      return base;
    }
    this.#seen.set(base, count + 1);
    return `${base}-${count + 1}`;
  }
}

/** The plain text of inline tokens: what a heading's anchor is made from. */
export function inlineText(tokens: readonly Token[]): string {
  let out = "";
  for (const token of tokens) {
    if ("tokens" in token && Array.isArray(token.tokens) && token.type !== "codespan") out += inlineText(token.tokens);
    else if (token.type === "codespan") out += (token as Tokens.Codespan).text;
    else if (token.type === "image") out += (token as Tokens.Image).text;
    else if (token.type === "br") out += " ";
    else if ("text" in token && typeof token.text === "string") out += token.text;
  }
  return out.replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'");
}

/** Every heading in a Markdown document, in order, with its GitHub anchor. */
export function headingsOf(markdown: string): { depth: number; text: string; slug: string }[] {
  const slugger = new Slugger();
  const out: { depth: number; text: string; slug: string }[] = [];
  const walk = (tokens: readonly Token[]) => {
    for (const token of tokens) {
      if (token.type === "heading") {
        const heading = token as Tokens.Heading;
        const text = inlineText(heading.tokens);
        out.push({ depth: heading.depth, text, slug: slugger.slug(text) });
      } else if (token.type === "blockquote") walk((token as Tokens.Blockquote).tokens);
    }
  };
  walk(new Marked().lexer(markdown));
  return out;
}

/** Every link and image target in a Markdown document, with the line it is on. */
export function targetsOf(markdown: string): { kind: "link" | "image"; href: string; line: number }[] {
  const out: { kind: "link" | "image"; href: string; line: number }[] = [];
  // Tokens are walked in document order, so each one is looked for after the last one found.
  let cursor = 0;
  const walk = (tokens: readonly Token[]) => {
    for (const token of tokens) {
      if (token.type === "link" || token.type === "image") {
        const at = markdown.indexOf(token.raw, cursor);
        if (at >= 0) cursor = at;
        out.push({ kind: token.type, href: (token as Tokens.Link).href, line: at < 0 ? 0 : markdown.slice(0, at).split("\n").length });
      }
      if ("tokens" in token && Array.isArray(token.tokens)) walk(token.tokens);
      if (token.type === "list") for (const item of (token as Tokens.List).items) walk(item.tokens);
      if (token.type === "table") {
        const table = token as Tokens.Table;
        for (const cell of table.header) walk(cell.tokens);
        for (const row of table.rows) for (const cell of row) walk(cell.tokens);
      }
    }
  };
  walk(new Marked().lexer(markdown));
  return out;
}

export interface HeadingInfo {
  depth: number;
  text: string;
  id: string;
  /** "3" for a chapter, "3.2" for a section; absent for anything deeper or unnumbered. */
  number?: string;
}

export interface RenderOptions {
  /** Prefixed to every anchor, so two chapters can both have a "Safety" section. */
  idPrefix: string;
  /** The chapter's number, when it is a numbered chapter. */
  chapter?: number;
  /** A link's target inside the one document ("#id"), or undefined to print its text only. */
  resolveLink: (href: string) => string | undefined;
  /** The URL an image is loaded from. */
  imageSrc: (href: string) => string;
}

export interface Rendered {
  html: string;
  title: string;
  headings: HeadingInfo[];
  figures: number;
}

const escapeHtml = (text: string): string => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

export function renderMarkdown(markdown: string, options: RenderOptions): Rendered {
  const slugger = new Slugger();
  const headings: HeadingInfo[] = [];
  let title = "";
  let section = 0;
  let figures = 0;
  const anchor = (slug: string, depth: number) => (depth === 1 ? options.idPrefix : `${options.idPrefix}--${slug}`);

  const marked = new Marked({
    gfm: true,
    renderer: {
      heading(token: Tokens.Heading): string {
        const text = inlineText(token.tokens);
        const id = anchor(slugger.slug(text), token.depth);
        const inner = this.parser.parseInline(token.tokens);
        const info: HeadingInfo = { depth: token.depth, text, id };
        let label = "";
        if (token.depth === 1) {
          title = text;
          if (options.chapter !== undefined) {
            info.number = String(options.chapter);
            label = `<span class="chapter-label">Chapter ${options.chapter}</span>`;
          }
        } else if (token.depth === 2 && options.chapter !== undefined) {
          section += 1;
          info.number = `${options.chapter}.${section}`;
          label = `<span class="secnum">${info.number}</span>`;
        }
        headings.push(info);
        return `<h${token.depth} id="${id}">${label}${inner}</h${token.depth}>\n`;
      },
      paragraph(token: Tokens.Paragraph): string {
        const meaningful = token.tokens.filter((t) => !(t.type === "text" && t.raw.trim() === ""));
        const only = meaningful[0];
        if (meaningful.length === 1 && only?.type === "image") {
          const image = only as Tokens.Image;
          figures += 1;
          const number = options.chapter === undefined ? `${figures}` : `${options.chapter}.${figures}`;
          return `<figure id="${options.idPrefix}--figure-${figures}"><img src="${escapeHtml(options.imageSrc(image.href))}" alt="${escapeHtml(image.text)}"><figcaption><span class="fignum">Figure ${number}.</span> ${escapeHtml(image.text)}</figcaption></figure>\n`;
        }
        return `<p>${this.parser.parseInline(token.tokens)}</p>\n`;
      },
      image(token: Tokens.Image): string {
        return `<img class="inline" src="${escapeHtml(options.imageSrc(token.href))}" alt="${escapeHtml(token.text)}">`;
      },
      link(token: Tokens.Link): string {
        const inner = this.parser.parseInline(token.tokens);
        if (/^(https?:|mailto:)/.test(token.href)) {
          const shown = inlineText(token.tokens);
          const bare = token.href.replace(/^mailto:/, "");
          const url = shown === token.href || shown === bare ? "" : `<span class="url"> (${escapeHtml(bare)})</span>`;
          return `<a href="${escapeHtml(token.href)}">${inner}</a>${url}`;
        }
        const target = options.resolveLink(token.href);
        return target === undefined ? `<span class="xref">${inner}</span>` : `<a class="xref" href="${escapeHtml(target)}">${inner}</a>`;
      },
      blockquote(token: Tokens.Blockquote): string {
        const kind = /^\s*\*\*(Warning|Danger)/i.test(token.text) ? "warning" : /^\s*\*\*Note/i.test(token.text) ? "note" : "callout";
        return `<aside class="${kind}">${this.parser.parse(token.tokens)}</aside>\n`;
      },
      table(token: Tokens.Table): string {
        const cell = (c: Tokens.TableCell, tag: "th" | "td") => {
          const align = c.align ? ` style="text-align:${c.align}"` : "";
          return `<${tag}${align}>${this.parser.parseInline(c.tokens)}</${tag}>`;
        };
        const head = `<thead><tr>${token.header.map((c) => cell(c, "th")).join("")}</tr></thead>`;
        const body = `<tbody>${token.rows.map((row) => `<tr>${row.map((c) => cell(c, "td")).join("")}</tr>`).join("")}</tbody>`;
        return `<table>${head}${body}</table>\n`;
      },
    },
  });

  const html = marked.parse(markdown, { async: false });
  return { html, title, headings, figures };
}
