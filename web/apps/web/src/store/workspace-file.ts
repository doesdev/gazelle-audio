// Workspace backup files. An export is the workspace document exactly as the server holds it; an
// import is checked here only for the shape a workspace must have, so a stray file gets a plain
// reason rather than the server's bare HTTP status. Everything past the shape (ids, slots, colours,
// links) is the server's validation, which decides when the file is sent. The parsed document is
// passed on untouched: fields this version does not know (a newer server's, say) are neither
// dropped nor defaulted here.

import type { Workspace } from "gazelle-audio-client";

/** What a file holds, for the confirmation before it replaces the workspace. */
export interface WorkspaceSummary {
  /** Device names. */
  names: number;
  /** Groups, nested ones included. */
  groups: number;
  links: number;
  /** Devices with a mixer layout. */
  mixers: number;
  /** Saved layouts. */
  layouts: number;
  /** Cross-device surfaces. */
  surfaces: number;
  /** Every device id the file mentions, sorted. */
  devices: string[];
}

export type WorkspaceFileRead = { ok: true; workspace: Workspace; summary: WorkspaceSummary } | { ok: false; problem: string };

/** `gazelle-workspace-YYYY-MM-DD.json`, by the local date. */
export function workspaceFileName(now: Date): string {
  const two = (n: number) => String(n).padStart(2, "0");
  return `gazelle-workspace-${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}.json`;
}

export function workspaceFileText(workspace: Workspace): string {
  return `${JSON.stringify(workspace, null, 2)}\n`;
}

const isMap = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * Reads a chosen file. `problem` completes "it …" ("is not JSON"). `readableVersion` is the version
 * of the workspace the server gave, so a file from a newer Gazelle is not handed to an older server,
 * which would quietly drop what it does not know.
 */
export function readWorkspaceFile(text: string, readableVersion: number): WorkspaceFileRead {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ok: false, problem: "is not JSON" };
  }
  if (!isMap(parsed)) return { ok: false, problem: "is not a Gazelle workspace" };
  const version = parsed["version"];
  if (typeof version !== "number" || !Number.isInteger(version) || version < 1) return { ok: false, problem: "has no version number, so it is not a Gazelle workspace" };
  if (version > readableVersion) return { ok: false, problem: `was written by a newer Gazelle (workspace version ${version}); this server reads version ${readableVersion}` };
  for (const part of ["groups", "links", "layouts", "surfaces"]) {
    if (part in parsed && !Array.isArray(parsed[part])) return { ok: false, problem: `has ${part} that are not a list` };
  }
  for (const part of ["aliases", "mixers", "device_colors"]) {
    if (part in parsed && !isMap(parsed[part])) return { ok: false, problem: `has ${part} that are not a map of devices` };
  }
  return { ok: true, workspace: parsed as unknown as Workspace, summary: summarise(parsed) };
}

function summarise(document: Record<string, unknown>): WorkspaceSummary {
  const devices = new Set<string>();
  const list = (part: string): unknown[] => (Array.isArray(document[part]) ? (document[part] as unknown[]) : []);
  const map = (part: string): Record<string, unknown> => (isMap(document[part]) ? document[part] : {});
  const members = (item: unknown) => {
    if (!isMap(item) || !Array.isArray(item["members"])) return;
    for (const member of item["members"]) if (isMap(member) && typeof member["device_id"] === "string") devices.add(member["device_id"]);
  };
  const countGroups = (groups: unknown[]): number =>
    groups.reduce<number>((total, group) => {
      members(group);
      return total + 1 + (isMap(group) && Array.isArray(group["children"]) ? countGroups(group["children"]) : 0);
    }, 0);

  const aliases = Object.keys(map("aliases"));
  const mixers = Object.keys(map("mixers"));
  for (const id of [...aliases, ...mixers, ...Object.keys(map("device_colors"))]) devices.add(id);
  const links = list("links");
  links.forEach(members);
  const surfaces = list("surfaces");
  for (const surface of surfaces) {
    if (!isMap(surface)) continue;
    if (isMap(surface["mixes"])) for (const id of Object.keys(surface["mixes"])) devices.add(id);
    if (Array.isArray(surface["strips"])) for (const strip of surface["strips"]) if (isMap(strip) && typeof strip["device_id"] === "string") devices.add(strip["device_id"]);
  }
  return { names: aliases.length, groups: countGroups(list("groups")), links: links.length, mixers: mixers.length, layouts: list("layouts").length, surfaces: surfaces.length, devices: [...devices].sort() };
}
