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

// Recall (workspace spec §2.3, phase 6). A **plan** is a description: the server reads every device
// fresh, compares, and answers the commands recall would send, in order, with the bytes each one
// would carry. Asking for a plan sends nothing to a device, and this client has no way to apply one.

/** One command a recall plan would send. */
export interface RecallStep {
  device_id: string;
  model: string;
  /** The opt-in part: `silence`, `clock`, `settings`, `dc_coupling`, `inputs`, `phantom`, `routing`, `mixer`, `outputs` or `restore`. */
  part: string;
  title: string;
  /** What a person reads: "Mixer · Mix 1 · strip 3". */
  label: string;
  command: string;
  ext3: number | null;
  args: Record<string, unknown>;
  /** The exact bytes, as the server's own dry run renders them. */
  bytes: string;
  bytes_len: number;
  /** The diff paths this one command puts back. */
  paths: string[];
  from?: unknown;
  to?: unknown;
  /** Anything to read before it is sent: "raises Monitor by 20 dB". */
  note?: string;
  /** The guards not satisfied. Empty means it would be sent as the plan stands. */
  blocked_by: string[];
}

/** A captured value recall will not put back, and why. */
export interface RecallExcluded {
  device_id: string;
  section: string;
  path: string;
  label: string;
  kind: "no_writer" | "withheld" | "unmapped" | "unreadable" | "incomplete" | "device_missing" | "not_chosen";
  command?: string;
  reason: string;
}

/** One opt-in part of a recall, as the preview draws it. */
export interface RecallPart {
  name: string;
  title: string;
  default_on: boolean;
  chosen: boolean;
  needs_confirming: boolean;
  confirmed: boolean;
  steps: number;
}

/** An output the snapshot would make louder by more than the plan's threshold. */
export interface RecallRaisedOutput {
  device_id: string;
  output: string;
  /** Where it is now, in dB of attenuation (96 is -inf), or null when nobody could read it. */
  now: number | null;
  /** Where the snapshot puts it. */
  snapshot: number;
  /** How far this raises it, or null when the present is unknown. */
  raised_db: number | null;
}

export interface RecallDevicePlan {
  device_id: string;
  model: string;
  family: string;
  missing: boolean;
  steps: number;
  excluded: number;
}

export interface RecallPlan {
  snapshot: SnapshotSummary;
  prepared_at: string;
  /** False in dry run, where the server answers no reads and nothing is known to be right already. */
  current_state_read: boolean;
  raise_threshold_db: number;
  parts: RecallPart[];
  devices: RecallDevicePlan[];
  steps: RecallStep[];
  excluded: RecallExcluded[];
  raised_outputs: RecallRaisedOutput[];
  /** Inputs the plan would switch 48V on for, by name. */
  phantom_on: string[];
  /** How many steps no guard is holding back. */
  ready: number;
  /** Workspace-side differences: layout only, and not steps. */
  workspace_changes: number;
  /** Always false: a plan is a description. */
  sent: boolean;
  note: string;
}

/** What a caller may choose when asking for a plan. Omitted, the server's defaults apply. */
export interface RecallAsk {
  parts?: Record<string, boolean>;
  confirm?: Record<string, boolean>;
  confirm_raised_outputs?: boolean;
  devices?: string[];
}
