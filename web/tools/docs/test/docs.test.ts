// The docs build's pure parts: the dash rule, anchors, figures, cross-references and the
// command-line comparison. Printing itself needs Chromium and is exercised by `pnpm docs`.

import assert from "node:assert/strict";
import { test } from "node:test";

import { findDashes } from "../src/checks.ts";
import { compareCli, flagsInHelp, flagsInTable } from "../src/cli.ts";
import { headingsOf, renderMarkdown, slugify, targetsOf } from "../src/markdown.ts";

const EM = String.fromCharCode(0x2014);
const EN = String.fromCharCode(0x2013);

test("an em dash and an en dash are both found, with their line and column", () => {
  assert.deepEqual(findDashes(`fine, and fine\nnot ${EM} this\n1${EN}2`), [
    { line: 2, column: 5, dash: "em dash (U+2014)" },
    { line: 3, column: 2, dash: "en dash (U+2013)" },
  ]);
  assert.deepEqual(findDashes("a hyphen - and a minus sign − are fine"), []);
});

test("anchors are GitHub's, and repeated headings are numbered", () => {
  assert.equal(slugify("48V phantom power, and its thump"), "48v-phantom-power-and-its-thump");
  assert.equal(slugify("The `--backend` flag"), "the---backend-flag");
  assert.deepEqual(
    headingsOf("# Title\n## Mute\n## Mute\n### Why?").map((h) => h.slug),
    ["title", "mute", "mute-1", "why"],
  );
});

test("links and images are found in paragraphs, lists and tables", () => {
  const md = "See [safety](02-safety.md#hard-mute).\n\n- [one](a.md)\n\n| a |\n|---|\n| [two](b.md) |\n\n![A figure](../images/x.png)\n";
  assert.deepEqual(
    targetsOf(md).map((t) => `${t.kind}:${t.href}:${t.line}`),
    ["link:02-safety.md#hard-mute:1", "link:a.md:3", "link:b.md:7", "image:../images/x.png:9"],
  );
});

test("a chapter numbers its h1 and h2, makes a lone image a captioned figure, and resolves cross-references", () => {
  const rendered = renderMarkdown("# Safety\n\n## Levels\n\n![The mixer at unity](m.png)\n\nSee [the glossary](17-glossary.md#dry-run) and [nowhere](../../README.md).\n\n### Detail\n", {
    idPrefix: "02-safety",
    chapter: 2,
    resolveLink: (href) => (href.startsWith("17-glossary.md") ? "#17-glossary--dry-run" : undefined),
    imageSrc: (href) => `file:///docs/images/${href}`,
  });
  assert.equal(rendered.title, "Safety");
  assert.deepEqual(
    rendered.headings.map((h) => [h.depth, h.id, h.number]),
    [
      [1, "02-safety", "2"],
      [2, "02-safety--levels", "2.1"],
      [3, "02-safety--detail", undefined],
    ],
  );
  assert.match(rendered.html, /<span class="chapter-label">Chapter 2<\/span>Safety/);
  assert.match(rendered.html, /<figure id="02-safety--figure-1"><img src="file:\/\/\/docs\/images\/m.png" alt="The mixer at unity"><figcaption><span class="fignum">Figure 2.1.<\/span> The mixer at unity<\/figcaption><\/figure>/);
  assert.match(rendered.html, /<a class="xref" href="#17-glossary--dry-run">the glossary<\/a>/);
  assert.match(rendered.html, /<span class="xref">nowhere<\/span>/);
});

test("a quote opening with Warning is a warning callout", () => {
  const rendered = renderMarkdown("> **Warning.** Turn it down first.\n\n> **Note.** Fine.\n", { idPrefix: "x", resolveLink: () => undefined, imageSrc: (h) => h });
  assert.match(rendered.html, /<aside class="warning">/);
  assert.match(rendered.html, /<aside class="note">/);
});

test("the command-line chapter is compared with --help both ways, defaults included", () => {
  const help = "Options:\n      --bind <BIND>\n          Address\n          [default: 127.0.0.1:8420]\n      --dry-run\n          Never write\n  -h, --help\n          Print help\n  -V, --version\n          Print version\n";
  assert.deepEqual(flagsInHelp(help), ["--bind", "--dry-run", "--help", "--version"]);
  const chapter = "| Flag | What |\n|---|---|\n| `--bind <ADDR>` | default `127.0.0.1:8420` |\n| `--dry-run` | x |\n| `--help`, `-h` | x |\n| `--version` | x |\n| `--gone` | x |\n";
  assert.deepEqual(flagsInTable(chapter), ["--bind", "--dry-run", "--help", "--version", "--gone"]);
  assert.deepEqual(compareCli(help, chapter), ["--gone is in the command-line table but the binary does not offer it"]);
  assert.deepEqual(compareCli(help, chapter.replace("127.0.0.1:8420", "localhost")), [
    "--gone is in the command-line table but the binary does not offer it",
    'the default "127.0.0.1:8420" from --help is not mentioned in the command-line chapter',
  ]);
});
