// Snapshots as the server keeps them (crates/gazelle-audio-server/src/snapshot): a named, dated
// record of the workspace and of every attached device's state, which can be compared with now and,
// later, recalled. Keys stay snake_case as on the wire.
//
// Nothing in a snapshot is written to a device by anything here: capture and compare only read.

import type { Workspace } from "./workspace.ts";

/** Schema version this client writes and reads; the server refuses anything newer. */
export const SNAPSHOT_VERSION = 1;

/** A value the device would not give up, and why, instead of a value invented for it. */
export interface Unreadable {
  /** Where it would have gone, as a diff path: `inputs.preamps`, `routing.SPDIF_OUT0`. */
  path: string;
  reason: string;
}

/** One device's state at the moment of capture, as decoded named values. */
export interface DeviceSnapshot {
  family: string;
  model: string;
  /** RFC 3339, UTC. */
  read_at: string;
  /** The preset slot the device said was current. Recorded, never recalled. */
  current_preset?: number | null;
  /** Section name (`inputs`, `mixer`, `routing`, `outputs`, `clock`, `settings`) to its values. */
  sections: Record<string, unknown>;
  unreadable: Unreadable[];
}

export interface Snapshot {
  version: number;
  id: string;
  name: string;
  /** RFC 3339, UTC. */
  created: string;
  note: string;
  workspace: Workspace;
  devices: Record<string, DeviceSnapshot>;
}

/** What one device contributed, as the list shows it. Values are not in a summary. */
export interface SnapshotDeviceSummary {
  device_id: string;
  family: string;
  model: string;
  read_at: string;
  current_preset: number | null;
  sections: string[];
  /** How many values could not be read. */
  unreadable: number;
}

export interface SnapshotSummary {
  version: number;
  id: string;
  name: string;
  created: string;
  note: string;
  devices: SnapshotDeviceSummary[];
}

/** Why a value appears in a diff. */
export type ChangeKind = "changed" | "only_in_snapshot" | "only_now" | "unknown";

export interface Change {
  /** The path inside the section, for recall to act on. */
  path: string;
  /** The same thing for a person: "Mixer · Mix 1 · strip 3 · level". */
  label: string;
  kind: ChangeKind;
  from?: unknown;
  to?: unknown;
  /** Why a value is unknown on one side or the other. */
  reason?: string;
}

export interface SectionDiff {
  section: string;
  title: string;
  changes: Change[];
}

export interface DeviceDiff {
  device_id: string;
  model: string;
  family: string;
  /** In the snapshot, not attached now. */
  missing: boolean;
  /** Attached now, not in the snapshot. */
  added: boolean;
  sections: SectionDiff[];
  changes: number;
}

export interface SnapshotDiff {
  snapshot: SnapshotSummary;
  compared_at: string;
  workspace: Change[];
  devices: DeviceDiff[];
  changes: number;
  same: boolean;
}

/** What `POST /snapshots/import` did with each snapshot in a backup. */
export interface SnapshotImport {
  added: string[];
  skipped: string[];
}
