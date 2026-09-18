// Workspace backup files. An export is the workspace document exactly as the server holds it; an
// import is checked here only for the shape a workspace must have, so a stray file gets a plain
// reason rather than the server's bare HTTP status. Everything past the shape (ids, slots, colours,
// links) is the server's validation, which decides when the file is sent. The parsed document is
// passed on untouched: fields this version does not know (a newer server's, say) are neither
// dropped nor defaulted here.

import type { Snapshot, Workspace } from "gazelle-audio-client";

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
  /** Declared digital cables. */
  cables: number;
  /** Cross-device surfaces. */
  surfaces: number;
  /** Devices with a choice of Control Room outputs. */
  controlRooms: number;
  /** Every device id the file mentions, sorted. */
  devices: string[];
}

export type WorkspaceFileRead = { ok: true; workspace: Workspace; summary: WorkspaceSummary; snapshots: Snapshot[] } | { ok: false; problem: string };

/** What a full backup calls itself, so a plain workspace is never mistaken for one. */
export const BACKUP_KIND = "gazelle-backup";
/** The backup wrapper's own version, separate from the workspace's and the snapshots'. */
export const BACKUP_VERSION = 1;

/** `gazelle-backup-YYYY-MM-DD.json`, by the local date. */
export function backupFileName(now: Date): string {
  return workspaceFileName(now).replace("gazelle-workspace-", "gazelle-backup-");
}

/** A full backup: the workspace as the server gave it, and every snapshot whole (spec §3.3). */
export function backupFileText(workspace: Workspace, snapshots: readonly Snapshot[]): string {
  return `${JSON.stringify({ kind: BACKUP_KIND, version: BACKUP_VERSION, workspace, snapshots }, null, 2)}\n`;
}

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
export function readWorkspaceFile(text: string, readableVersion: number, readableSnapshotVersion = 1): WorkspaceFileRead {
  let document: unknown;
  try {
    document = JSON.parse(text);
  } catch {
    return { ok: false, problem: "is not JSON" };
  }
  if (!isMap(document)) return { ok: false, problem: "is not a Gazelle workspace" };

  // A full backup wraps the workspace and carries snapshots beside it (spec §3.3); a plain workspace
  // export still imports, so neither file has to be told apart by its name.
  let parsed: Record<string, unknown> = document;
  let snapshots: Snapshot[] = [];
  if (document["kind"] === BACKUP_KIND) {
    const wrapper = document["version"];
    if (typeof wrapper !== "number" || wrapper > BACKUP_VERSION) return { ok: false, problem: `is a backup written by a newer Gazelle (backup version ${String(wrapper)})` };
    if (!isMap(document["workspace"])) return { ok: false, problem: "is a backup with no workspace in it" };
    parsed = document["workspace"];
    const held = document["snapshots"];
    if (held !== undefined && !Array.isArray(held)) return { ok: false, problem: "is a backup whose snapshots are not a list" };
    for (const snapshot of (held ?? []) as unknown[]) {
      if (!isMap(snapshot) || typeof snapshot["id"] !== "string" || typeof snapshot["name"] !== "string") return { ok: false, problem: "is a backup holding something that is not a snapshot" };
      const version = snapshot["version"];
      if (typeof version !== "number" || !Number.isInteger(version) || version < 1) return { ok: false, problem: "is a backup holding a snapshot with no version number" };
      if (version > readableSnapshotVersion) return { ok: false, problem: `holds a snapshot written by a newer Gazelle (snapshot version ${version}); this server reads version ${readableSnapshotVersion}` };
    }
    snapshots = (held ?? []) as Snapshot[];
  }
  const version = parsed["version"];
  if (typeof version !== "number" || !Number.isInteger(version) || version < 1) return { ok: false, problem: "has no version number, so it is not a Gazelle workspace" };
  if (version > readableVersion) return { ok: false, problem: `was written by a newer Gazelle (workspace version ${version}); this server reads version ${readableVersion}` };
  for (const part of ["groups", "links", "layouts", "surfaces", "cables"]) {
    if (part in parsed && !Array.isArray(parsed[part])) return { ok: false, problem: `has ${part} that are not a list` };
  }
  for (const part of ["aliases", "mixers", "device_colors", "control_room"]) {
    if (part in parsed && !isMap(parsed[part])) return { ok: false, problem: `has ${part} that are not a map of devices` };
  }
  return { ok: true, workspace: parsed as unknown as Workspace, summary: summarise(parsed), snapshots };
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
  const controlRooms = Object.keys(map("control_room"));
  for (const id of [...aliases, ...mixers, ...Object.keys(map("device_colors")), ...controlRooms]) devices.add(id);
  const links = list("links");
  links.forEach(members);
  const surfaces = list("surfaces");
  for (const surface of surfaces) {
    if (!isMap(surface)) continue;
    if (isMap(surface["mixes"])) for (const id of Object.keys(surface["mixes"])) devices.add(id);
    if (Array.isArray(surface["strips"])) for (const strip of surface["strips"]) if (isMap(strip) && typeof strip["device_id"] === "string") devices.add(strip["device_id"]);
  }
  const cables = list("cables");
  for (const cable of cables) for (const end of isMap(cable) ? [cable["from"], cable["to"]] : []) if (isMap(end) && typeof end["device_id"] === "string") devices.add(end["device_id"]);
  return { names: aliases.length, groups: countGroups(list("groups")), links: links.length, mixers: mixers.length, layouts: list("layouts").length, surfaces: surfaces.length, cables: cables.length, controlRooms: controlRooms.length, devices: [...devices].sort() };
}
