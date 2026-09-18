// The command-line chapter must agree with the binary's own `--help`: every flag the binary
// offers is in the chapter's table, every flag in the table is one the binary offers, and every
// default the binary prints appears in the chapter. The code is the truth; this says where the
// words have drifted from it.

import { spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

/** The long flags `--help` lists, in its order. */
export function flagsInHelp(help: string): string[] {
  const out: string[] = [];
  for (const match of help.matchAll(/^\s+(?:-\w, )?--([a-z][a-z0-9-]*)/gm)) {
    const flag = `--${match[1]}`;
    if (!out.includes(flag)) out.push(flag);
  }
  return out;
}

/** The defaults `--help` prints, as `[default: …]`. */
export function defaultsInHelp(help: string): string[] {
  return [...help.matchAll(/\[default: ([^\]]+)\]/g)].map((m) => m[1]!.trim());
}

/** The flags a Markdown table documents: rows whose first cell starts with a `--flag`. */
export function flagsInTable(markdown: string): string[] {
  const out: string[] = [];
  for (const match of markdown.matchAll(/^\|\s*`(--[a-z][a-z0-9-]*)/gm)) {
    if (!out.includes(match[1]!)) out.push(match[1]!);
  }
  return out;
}

export function compareCli(help: string, chapter: string): string[] {
  const problems: string[] = [];
  const offered = flagsInHelp(help);
  const documented = flagsInTable(chapter);
  for (const flag of offered) if (!documented.includes(flag)) problems.push(`${flag} is in --help but not in the command-line table`);
  for (const flag of documented) if (!offered.includes(flag)) problems.push(`${flag} is in the command-line table but the binary does not offer it`);
  for (const value of defaultsInHelp(help)) {
    // clap prints a list default space-separated; the chapter may write it with commas.
    const variants = [value, value.replace(/ /g, ",")];
    if (!variants.some((v) => chapter.includes(v))) problems.push(`the default "${value}" from --help is not mentioned in the command-line chapter`);
  }
  return problems;
}

/** The most recently built server binary, or the one GAZELLE_BIN names. */
export function findBinary(repoRoot: string): string | undefined {
  const named = process.env["GAZELLE_BIN"];
  if (named) return existsSync(named) ? named : undefined;
  const target = resolve(repoRoot, process.env["CARGO_TARGET_DIR"] ?? "target");
  const exe = process.platform === "win32" ? "gazelle-audio-server.exe" : "gazelle-audio-server";
  const candidates = ["debug", "release"].map((profile) => join(target, profile, exe)).filter((path) => existsSync(path));
  candidates.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  return candidates[0];
}

export function helpOf(binary: string): string {
  // --help starts nothing, but the refusal is set anyway: this is a harness like any other.
  const result = spawnSync(binary, ["--help"], { encoding: "utf8", env: { ...process.env, GAZELLE_NO_HARDWARE: "1" } });
  if (result.status !== 0) throw new Error(`${binary} --help failed: ${result.stderr || result.error?.message}`);
  return result.stdout;
}
